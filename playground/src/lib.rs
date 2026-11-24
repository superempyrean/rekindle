//! The theme playground's engine side.
//!
//! A [`Session`] holds a wire capture and replays its turns through
//! [`rekindle_wire_inject::Engine`], the engine rekindle's host runs on the
//! client's wire. A theme becomes the engine's [`Work`], set with
//! [`Engine::set_work`] and brought into the host's tree with
//! [`Engine::flush`], as the host does when a plugin changes the theme. The
//! page paints the host's tree ([`Session::tree_json`]), so the colours it
//! shows are the ones the engine writes on the client.
//!
//! Built for `wasm32-unknown-unknown`, the crate exports the session to the
//! page through [`abi`]. [`sanitize`] makes fixtures out of captures.

#[cfg(target_arch = "wasm32")]
pub mod abi;
pub mod sanitize;
pub mod theme;
mod tree_json;

use rekindle_wire::capture::{self, Record};
use rekindle_wire_inject::{Engine, Work};

pub use tree_json::{tree_json, value_json};

/// A capture replayed through the engine, up to one of its turns.
pub struct Session {
    records: Vec<Record>,
    /// How many of `records` the engine has applied.
    upto: usize,
    engine: Engine,
    /// Turns the engine refused, by index.
    errors: Vec<(usize, String)>,
}

impl Session {
    /// A session over the turns in `capture`, all of them applied, with no
    /// theme.
    pub fn new(capture: &[u8]) -> Result<Session, String> {
        let (records, _) = capture::parse(capture).map_err(|e| e.to_string())?;
        let mut s = Session {
            upto: 0,
            engine: Engine::new(Work::default()).map_err(|e| e.0)?,
            errors: Vec::new(),
            records,
        };
        s.replay(s.records.len());
        Ok(s)
    }

    /// The number of turns in the capture.
    pub fn turns(&self) -> usize {
        self.records.len()
    }

    /// How many turns are applied.
    pub fn upto(&self) -> usize {
        self.upto
    }

    /// The turns the engine refused, and why.
    pub fn errors(&self) -> &[(usize, String)] {
        &self.errors
    }

    /// Apply the first `n` turns, from a fresh engine that keeps the work.
    pub fn replay(&mut self, n: usize) {
        let n = n.min(self.records.len());
        let work = self.engine.work().clone();
        self.engine = Engine::new(work).expect("the engine accepted this work before");
        self.errors.clear();
        for (i, r) in self.records[..n].iter().enumerate() {
            if let Err(e) = self.engine.filter(&r.bytes) {
                self.errors.push((i, e.to_string()));
            }
        }
        self.upto = n;
    }

    /// Replace the engine's work and bring the host's tree in line with it.
    pub fn set_work(&mut self, work: Work) -> Result<(), String> {
        self.engine.set_work(work).map_err(|e| e.0)?;
        self.engine.flush();
        Ok(())
    }

    /// [`set_work`](Self::set_work) from a `Work` in rekindle-wire-inject's
    /// JSON.
    pub fn set_work_json(&mut self, json: &str) -> Result<(), String> {
        let work: Work = serde_json::from_str(json).map_err(|e| format!("work: {e}"))?;
        self.set_work(work)
    }

    /// Apply a theme (see [`theme`]) and report each bound token's colour
    /// and the recolours, as JSON.
    pub fn set_theme_json(&mut self, json: &str) -> Result<String, String> {
        let theme = theme::Theme::from_json(json)?;
        self.set_work(theme.work())?;
        Ok(theme.report().to_string())
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The guest's tree (`mirror` false) or the host's, as JSON for the
    /// page to paint.
    pub fn tree_json(&self, mirror: bool) -> String {
        tree_json(if mirror {
            self.engine.mirror()
        } else {
            self.engine.guest()
        })
    }
}

#[cfg(test)]
mod tests;
