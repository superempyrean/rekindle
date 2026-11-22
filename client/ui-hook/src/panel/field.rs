//! Text fields, and the value each panel element has on rekindle's side
//! of the plugin.

use std::sync::Mutex;

use rekindle_native::{Ctx, Cursor, Edges, Node, Rgba, Style, client};

use super::{BUTTON_BG, FIELD_BG, FIELD_BORDER, UiInput, column, muted, report_ui, text};

/// What had focus before a rekindle text field took it: Enter and Escape
/// give it back.
static FIELD_RETURN: Mutex<Option<u64>> = Mutex::new(None);

thread_local! {
    /// Focus handles for text fields, kept for the process by element key.
    static FIELD_FOCUS: std::cell::RefCell<std::collections::HashMap<String, client::Focus>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    /// Optimistic local values by element key. A plugin needn't re-show
    /// its panel for an edit to stick; when it does show a new value,
    /// that wins.
    static LOCAL: std::cell::RefCell<std::collections::HashMap<String, Local>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// One element's value on rekindle's side of the plugin.
struct Local {
    /// The last value the plugin showed.
    shown: String,
    /// The value the user has now.
    current: String,
    /// Values reported to the plugin that it hasn't shown back yet, oldest
    /// first. The plugin answers each edit by re-showing the panel, and
    /// those answers trail the user's typing: one of these coming back is
    /// an echo, not a change.
    sent: Vec<String>,
}

/// The text field gpui has focused, by element key.
///
/// # Safety
///
/// `window` is the client's live `Window`, on its UI thread.
pub(crate) unsafe fn focused_field(window: *const u8) -> Option<String> {
    FIELD_FOCUS.with(|f| {
        f.borrow()
            .iter()
            // SAFETY: the caller's contract.
            .find(|(_, h)| unsafe { client::is_focused(window, h) })
            .map(|(k, _)| k.clone())
    })
}

/// A field's current (local) value.
pub(crate) fn field_value(key: &str) -> Option<String> {
    LOCAL.with(|l| l.borrow().get(key).map(|e| e.current.clone()))
}

pub(super) fn local_value(key: &str, shown: &str) -> String {
    LOCAL.with(|l| {
        let mut l = l.borrow_mut();
        let e = l.entry(key.to_string()).or_insert_with(|| Local {
            shown: shown.to_string(),
            current: shown.to_string(),
            sent: Vec::new(),
        });
        if e.shown != shown {
            match e.sent.iter().position(|v| v == shown) {
                // The plugin caught up to an edit: keep what the user has
                // typed since.
                Some(i) => {
                    e.sent.drain(..=i);
                    e.shown = shown.to_string();
                }
                None => {
                    *e = Local {
                        shown: shown.to_string(),
                        current: shown.to_string(),
                        sent: Vec::new(),
                    }
                }
            }
        }
        e.current.clone()
    })
}

pub(super) fn set_local(key: &str, value: &str) {
    LOCAL.with(|l| {
        if let Some(e) = l.borrow_mut().get_mut(key) {
            e.current = value.to_string();
            e.sent.push(value.to_string());
        }
    });
}

/// A minimal single-line text field, as gpui apps build them: a focusable
/// element (gpui focus: click focuses it, key presses arrive while it's
/// focused) showing its text and a caret. Backspace, Ctrl+V to paste,
/// Enter/Escape to leave. Each edit is reported to the plugin as
/// `Text(new value)`. No IME or selection.
pub(super) fn text_field(
    plugin: &str,
    panel: &str,
    id: &str,
    label: &str,
    value: &str,
    placeholder: &str,
    ctx: &mut Ctx,
) -> Node {
    let key = format!("{plugin}/{panel}/{id}");
    let (p, n, i) = (plugin.to_string(), panel.to_string(), id.to_string());
    text_field_with(&key, label, value, placeholder, ctx, move |text| {
        report_ui(&p, &n, &i, UiInput::Text(text));
    })
}

/// [`text_field`] with what an edit does: `edited` gets the field's new
/// text after each one.
pub(super) fn text_field_with(
    key: &str,
    label: &str,
    value: &str,
    placeholder: &str,
    ctx: &mut Ctx,
    edited: impl Fn(String) + Send + Sync + 'static,
) -> Node {
    let key = key.to_string();
    let focus = FIELD_FOCUS.with(|f| {
        let mut f = f.borrow_mut();
        if let Some(h) = f.get(&key) {
            return Some(*h);
        }
        // SAFETY: rekindle-native builds elements with the live `App`, on
        // the UI thread.
        let h = unsafe { client::new_focus(ctx.cx()) }?;
        f.insert(key.clone(), h);
        Some(h)
    });
    let focused = focus.is_some_and(|h| ctx.is_focused(&h));
    let current = local_value(&key, value);
    let shown = if current.is_empty() && !focused {
        muted(placeholder.to_string())
    } else {
        text(if focused {
            format!("{current}▏")
        } else {
            current.clone()
        })
    };
    let mut field = Node::div(
        Style {
            padding: Edges::xy(Some(8.0), Some(4.0)),
            radius: Some(6.0),
            background: Some(Rgba::hex(FIELD_BG)),
            border: Some((
                1.0,
                Rgba::hex(if focused { BUTTON_BG } else { FIELD_BORDER }),
            )),
            cursor: Some(Cursor::Text),
            line_height: Some(20.0),
            ..Default::default()
        },
        vec![shown],
    );
    if let Some(h) = focus {
        let k1 = key.clone();
        field = field
            .on_click_cx(move |ctx| {
                // Remember what had focus, to give it back on Enter.
                if !ctx.is_focused(&h) {
                    *FIELD_RETURN.lock().unwrap() = ctx.focused_id();
                }
                ctx.focus(&h);
                ctx.refresh();
            })
            .focusable(h, move |ctx, ks| {
                let mut cur = LOCAL.with(|l| {
                    l.borrow()
                        .get(&k1)
                        .map(|e| e.current.clone())
                        .unwrap_or_default()
                });
                match ks.key.as_str() {
                    "backspace" => {
                        cur.pop();
                    }
                    "enter" | "escape" => {
                        // Back to what had focus before (the client's
                        // own actions dispatch from there), or none.
                        match FIELD_RETURN.lock().unwrap().take() {
                            Some(id) if ctx.focus_id(id).is_ok() => {}
                            _ => ctx.blur(),
                        }
                        ctx.stop_propagation();
                        ctx.refresh();
                        return;
                    }
                    "v" if ks.control || ks.platform => {
                        if let Some(t) = crate::clipboard::read() {
                            cur.push_str(&t.replace(['\n', '\r'], " "));
                        }
                    }
                    _ if ks.control || ks.platform || ks.alt => return,
                    _ => match &ks.key_char {
                        Some(c) if !c.chars().any(char::is_control) => cur.push_str(c),
                        _ => return,
                    },
                }
                set_local(&k1, &cur);
                edited(cur);
                ctx.stop_propagation();
                ctx.refresh();
            });
    }
    if label.is_empty() {
        return field;
    }
    column(4.0, vec![muted(label.to_string()), field])
}
