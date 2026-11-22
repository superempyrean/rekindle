//! Plugins' message transforms, between the client's message rows and
//! rekindle's worker.
//!
//! The client asks its message decorators how each row it draws looks.
//! rekindle's decorator never waits for the plugins: it answers with the
//! look the worker last rendered for that message, and leaves a request
//! for the message as it is now ([`look_for`]). The worker renders the
//! requests it finds each tick ([`wanted`], [`set_looks`]). The client
//! redraws every frame while rekindle runs, so a row shows its rendered
//! look a frame or two after it first appears. The plugin runtime caches
//! what each renderer returned, so a row that didn't change costs no plugin
//! call.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use crate::client::{MessageRead, UserRead};

/// A row the client drew: the message and what a renderer is told about it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RenderRequest {
    pub guild: Option<u64>,
    pub channel: u64,
    pub message: MessageRead,
    pub author: Option<UserRead>,
    pub author_roles: Vec<u64>,
    pub grouped: bool,
}

impl RenderRequest {
    /// What the look depends on: a row whose key changed is rendered again.
    pub fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }
}

/// A badge after the author's name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LookBadge {
    pub text: String,
    pub tooltip: String,
    /// `0xRRGGBBAA`; 0 is the client's own colour.
    pub color: u32,
}

/// How the plugins' renderers change one row, in the client's terms.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub badges: Vec<LookBadge>,
    /// Byte ranges of the body, and their colour (`0xRRGGBBAA`, 0: the
    /// client's highlight colour).
    pub highlights: Vec<(usize, usize, u32)>,
    pub name_color: u32,
    pub tint: u32,
    pub append: String,
    pub replace: String,
    pub collapse: String,
    pub blur: bool,
    /// The plugins that changed the row, by name.
    pub by: Vec<String>,
}

impl Look {
    pub fn is_empty(&self) -> bool {
        *self == Look::default()
    }
}

/// Rows drawn since the worker last looked, by message id.
static WANTED: Mutex<Option<HashMap<u64, RenderRequest>>> = Mutex::new(None);
/// The latest look per message id, with the request key it was rendered
/// for.
static LOOKS: Mutex<Option<HashMap<u64, (u64, Look)>>> = Mutex::new(None);

/// UI thread: the look rendered for this row as it is now, if the worker
/// has one, and a request for it either way.
pub(crate) fn look_for(req: RenderRequest) -> Option<Look> {
    let key = req.key();
    let id = req.message.id;
    WANTED
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id, req);
    let looks = LOOKS.lock().unwrap();
    match looks.as_ref()?.get(&id) {
        Some((k, look)) if *k == key => Some(look.clone()),
        _ => None,
    }
}

/// Worker: the rows the client drew since the last call.
pub fn wanted() -> Vec<RenderRequest> {
    WANTED
        .lock()
        .unwrap()
        .take()
        .map(|m| m.into_values().collect())
        .unwrap_or_default()
}

/// Worker: the looks rendered for `rows` (a request and its look each).
pub fn set_looks(rows: Vec<(RenderRequest, Look)>) {
    let mut looks = LOOKS.lock().unwrap();
    let looks = looks.get_or_insert_with(HashMap::new);
    for (req, look) in rows {
        looks.insert(req.message.id, (req.key(), look));
    }
}

/// What rekindle wrote into the client's rows: the badges' texts (each
/// once) and how many highlights. For the self-test.
static WRITTEN: Mutex<(Vec<String>, usize)> = Mutex::new((Vec::new(), 0));

/// UI thread: `look` went into a row the client is drawing.
pub(crate) fn note_written(look: &Look) {
    let mut w = WRITTEN.lock().unwrap();
    for b in &look.badges {
        if !w.0.contains(&b.text) {
            w.0.push(b.text.clone());
        }
    }
    w.1 += look.highlights.len();
}

/// The badges rekindle put in the client's rows, and the highlights.
pub fn written() -> (Vec<String>, usize) {
    WRITTEN.lock().unwrap().clone()
}

/// How many rows have a non-empty look: for the self-test.
pub fn decorated_rows() -> usize {
    LOOKS.lock().unwrap().as_ref().map_or(0, |l| {
        l.values().filter(|(_, look)| !look.is_empty()).count()
    })
}

/// The badges rendered so far, as text: for the self-test.
pub fn badge_texts() -> Vec<String> {
    LOOKS
        .lock()
        .unwrap()
        .as_ref()
        .map(|l| {
            l.values()
                .flat_map(|(_, look)| look.badges.iter().map(|b| b.text.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(body: &str) -> RenderRequest {
        RenderRequest {
            guild: Some(1),
            channel: 2,
            message: MessageRead {
                id: 3,
                author: 4,
                body: body.into(),
                edited: false,
                pinned: false,
            },
            author: None,
            author_roles: vec![5],
            grouped: false,
        }
    }

    #[test]
    fn a_row_shows_the_look_rendered_for_it_as_it_is() {
        assert_eq!(look_for(row("hi")), None, "nothing rendered yet");
        let asked = wanted();
        assert_eq!(asked, vec![row("hi")]);
        let look = Look {
            tint: 0xff00_00ff,
            ..Look::default()
        };
        set_looks(vec![(asked[0].clone(), look.clone())]);
        assert_eq!(look_for(row("hi")), Some(look));
        assert_eq!(
            look_for(row("hi, edited")),
            None,
            "an edited row waits for its own render"
        );
        assert_eq!(wanted().len(), 1, "asked once per message");
    }
}
