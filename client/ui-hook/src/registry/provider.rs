//! rekindle's provider, and the client's values it builds.
//!
//! The client's providers are `Box<dyn Trait>`s, and so is rekindle's: the
//! compiler builds its vtables for mirror traits that declare the client's
//! methods in the client's order, with the same argument shapes (`*const
//! u8` for a client reference, `&str` for one, `u64`) and return types of
//! the same size (`[usize; 3]` for a `String` or `Vec`, `[u8; ACTION]` for
//! its action enum, all returned through a hidden pointer). The values they
//! return are the client's own types, built as bytes at recovered offsets
//! and allocated with this DLL's allocator, the process heap the client
//! frees with.

use std::sync::atomic::Ordering;

use super::contribution::{self, MENU_INVOKES, MenuTargetRef, PROFILE_OPENS};
use super::{RegistryLayout, layout};
use crate::{BUTTON_CLICKS, Click, VecLayout, queue_click, split_key};

/// The client's action enum's size, as its builtin providers return it. The
/// mirror traits return `[u8; ACTION]`; the host checks the recovered size.
pub const ACTION: usize = 32;

/// A chat-bar button's action: `Rc<dyn Fn(&ChatModel) -> ClientAction>`.
pub type ButtonAction = std::rc::Rc<dyn Fn(*const u8) -> [u8; ACTION]>;

/// Writes `bytes` into `buf` at `at`.
pub(super) fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Writes machine words into `buf` at `at`.
pub(super) fn put_words(buf: &mut [u8], at: usize, words: &[usize]) {
    for (i, w) in words.iter().enumerate() {
        put(buf, at + 8 * i, &w.to_ne_bytes());
    }
}

/// An owned `String` of `text`, as its three words: the client takes
/// ownership, and frees it.
pub(super) fn string(text: &str) -> [usize; 3] {
    // SAFETY: a `String` is three words with no padding, the representation
    // the client's `String` has (same compiler). The words carry its
    // allocation to the client.
    unsafe { std::mem::transmute::<String, [usize; 3]>(text.to_owned()) }
}

/// A `Vec` header: `cap`, `ptr`, `len` at their recovered slots.
fn header(v: &VecLayout, cap: usize, ptr: usize, len: usize) -> [usize; 3] {
    let mut h = [0usize; 3];
    h[v.cap_off / 8] = cap;
    h[v.ptr_off / 8] = ptr;
    h[v.len_off / 8] = len;
    h
}

/// An empty `Vec` (`Vec::new()`: no allocation, a dangling aligned pointer).
fn empty(v: &VecLayout) -> [usize; 3] {
    header(v, 0, 8, 0)
}

/// A `Vec` of `len` elements whose bytes, one after the other, are `bytes`,
/// as its header words.
fn vec_of(v: &VecLayout, bytes: &[u8], len: usize) -> [usize; 3] {
    if len == 0 {
        return empty(v);
    }
    let p = rekindle_native::mem::alloc(bytes.len(), 8, false);
    // SAFETY: `p` is a fresh allocation of `bytes.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    header(v, len, p as usize, len)
}

/// The client's "do nothing" action.
fn no_action(l: &RegistryLayout) -> [u8; ACTION] {
    let mut a = [0u8; ACTION];
    a[..8].copy_from_slice(&l.action_none_tag.to_ne_bytes());
    a
}

// The mirror traits: the client's, method for method. Only the client calls
// them, through the vtables, so rustc sees no caller.

/// `QuickSwitcherProvider { section, rows, confirm }`.
#[expect(dead_code, reason = "only the client calls these, through the vtable")]
pub(super) trait SwitcherProvider {
    fn section(&self) -> [usize; 3];
    fn rows(&self, model: *const u8) -> [usize; 3];
    fn confirm(&self, key: &str) -> [u8; ACTION];
}

/// `ChatBarProvider { buttons }`.
#[expect(dead_code, reason = "only the client calls these, through the vtable")]
pub(super) trait ChatBarProvider {
    fn buttons(&self, model: *const u8) -> [usize; 3];
}

/// `ContextMenuProvider { groups, invoke }`.
#[expect(dead_code, reason = "only the client calls these, through the vtable")]
pub(super) trait ContextMenuProvider {
    fn groups(&self, model: *const u8, target: *const u8) -> [usize; 3];
    fn invoke(&self, model: *const u8, key: &str, target: *const u8) -> [u8; ACTION];
}

/// `ProfileSectionProvider { opened, sections }`.
#[expect(dead_code, reason = "only the client calls these, through the vtable")]
pub(super) trait ProfileSectionProvider {
    fn opened(&self, model: *const u8, user: u64);
    fn sections(&self, model: *const u8, user: u64) -> [usize; 3];
}

/// `ModalProvider { modals, dismiss }`.
#[expect(dead_code, reason = "only the client calls these, through the vtable")]
pub(super) trait ModalProvider {
    fn modals(&self) -> [usize; 3];
    fn dismiss(&self, key: &str);
}

/// `MessageDecorator { decorate }`.
#[expect(dead_code, reason = "only the client calls these, through the vtable")]
pub(super) trait MessageDecorator {
    fn decorate(
        &self,
        model: *const u8,
        guild: Option<u64>,
        channel: u64,
        message: *const u8,
        grouped: bool,
        look: *mut u8,
    );
}

/// rekindle's provider: one value implementing every registry's trait.
pub(super) struct Rekindle;

impl SwitcherProvider for Rekindle {
    fn section(&self) -> [usize; 3] {
        let name = contribution::commands()
            .first()
            .map_or_else(|| "extensions".into(), |c| c.section.clone());
        string(&name)
    }

    fn rows(&self, _model: *const u8) -> [usize; 3] {
        let Some(l) = layout() else { return [0; 3] };
        let commands = contribution::commands();
        let mut rows = vec![0u8; commands.len() * l.row_size];
        for (c, b) in commands.iter().zip(rows.chunks_mut(l.row_size)) {
            put_words(b, l.row_key, &string(&c.key));
            put_words(b, l.row_glyph, &string(&c.glyph));
            put_words(b, l.row_title, &string(&c.title));
            put_words(b, l.row_detail, &string(&c.detail));
        }
        vec_of(&l.vec, &rows, commands.len())
    }

    fn confirm(&self, key: &str) -> [u8; ACTION] {
        let (plugin, id) = split_key(key);
        queue_click(Click::Command { plugin, id });
        layout().map_or([0; ACTION], |l| no_action(&l))
    }
}

impl ChatBarProvider for Rekindle {
    fn buttons(&self, model: *const u8) -> [usize; 3] {
        // The client asks for its chat-bar buttons every time it draws the
        // composer: the earliest and steadiest look at its model.
        crate::client::saw_model(model);
        let Some(l) = layout() else { return [0; 3] };
        let none = no_action(&l);
        let buttons = contribution::buttons();
        let mut bytes = vec![0u8; buttons.len() * l.button_size];
        for (i, (btn, b)) in buttons
            .iter()
            .zip(bytes.chunks_mut(l.button_size))
            .enumerate()
        {
            put_words(b, l.button_id, &string(&format!("plugin-{}", btn.key)));
            put_words(b, l.button_label, &string(&btn.glyph));
            put_words(b, l.button_tooltip, &string(&btn.tooltip));
            // After the client's own (it sorts by order).
            put(b, l.button_order, &(1000 + i as i32).to_ne_bytes());
            let key = btn.key.clone();
            let action: ButtonAction = std::rc::Rc::new(move |_model| {
                let (plugin, item) = split_key(&key);
                queue_click(Click::Button { plugin, item });
                BUTTON_CLICKS.fetch_add(1, Ordering::SeqCst);
                none
            });
            // SAFETY: an `Rc<dyn Fn>` is one fat pointer, data then vtable,
            // the representation the client's has (same compiler). The words
            // carry the `Rc`'s count to the client.
            let fat = unsafe { std::mem::transmute::<ButtonAction, [usize; 2]>(action) };
            put_words(b, l.button_action, &fat);
        }
        vec_of(&l.vec, &bytes, buttons.len())
    }
}

/// What the client's `MenuTarget` at `t` names, if it's a kind rekindle
/// knows.
///
/// # Safety
/// `t` must be a live `MenuTarget` of the client's.
unsafe fn read_target(l: &RegistryLayout, t: *const u8) -> Option<MenuTargetRef> {
    let t = t.addr();
    let read = |off: usize| -> u64 {
        // SAFETY: the caller's live target, with its tag and ids at these
        // offsets (the layout's `menu_target_*`).
        unsafe { rekindle_native::mem::read::<u64>(t + off) }
    };
    let tag = read(0) & 0xff;
    let id = read(l.target_id);
    Some(if tag == l.target_tag_message {
        MenuTargetRef::Message {
            id,
            channel: read(l.target_message_channel),
            author: read(l.target_message_author),
        }
    } else if tag == l.target_tag_user {
        MenuTargetRef::User { id }
    } else if tag == l.target_tag_channel {
        MenuTargetRef::Channel { id }
    } else {
        return None;
    })
}

impl ContextMenuProvider for Rekindle {
    fn groups(&self, _model: *const u8, target: *const u8) -> [usize; 3] {
        let Some(l) = layout() else { return [0; 3] };
        // SAFETY: the client passes the target its menu was opened on.
        let Some(t) = (unsafe { read_target(&l, target) }) else {
            return empty(&l.vec);
        };
        let items: Vec<_> = contribution::menu_items()
            .into_iter()
            .filter(|m| m.kind == t.kind())
            .collect();
        if items.is_empty() {
            return empty(&l.vec);
        }
        let mut rows = vec![0u8; items.len() * l.menu_row_size];
        for (m, b) in items.iter().zip(rows.chunks_mut(l.menu_row_size)) {
            put_words(b, l.menu_row_key, &string(&m.key));
            put_words(b, l.menu_row_label, &string(&m.label));
            b[l.menu_row_danger] = u8::from(m.danger);
        }
        // One group (a `Vec<MenuRow>`): the client draws a separator before it.
        let group = vec_of(&l.vec, &rows, items.len());
        let mut groups = [0u8; 24];
        put_words(&mut groups, 0, &group);
        vec_of(&l.vec, &groups, 1)
    }

    fn invoke(&self, _model: *const u8, key: &str, target: *const u8) -> [u8; ACTION] {
        let Some(l) = layout() else {
            return [0; ACTION];
        };
        MENU_INVOKES.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the client passes the target its menu was opened on.
        if let Some(target) = unsafe { read_target(&l, target) } {
            let (plugin, item) = split_key(key);
            queue_click(Click::Menu {
                plugin,
                item,
                target,
            });
        }
        no_action(&l)
    }
}

impl ProfileSectionProvider for Rekindle {
    fn opened(&self, _model: *const u8, user: u64) {
        PROFILE_OPENS.fetch_add(1, Ordering::SeqCst);
        queue_click(Click::Profile { user });
    }

    fn sections(&self, _model: *const u8, _user: u64) -> [usize; 3] {
        let Some(l) = layout() else { return [0; 3] };
        let sections = contribution::profiles();
        let mut bytes = vec![0u8; sections.len() * l.profile_size];
        for (item, b) in sections.iter().zip(bytes.chunks_mut(l.profile_size)) {
            put_words(
                b,
                l.profile_id,
                &string(&format!("{}/{}", item.plugin, item.id)),
            );
            put_words(b, l.profile_title, &string(&item.title));
            // `ProfileBody::Render`: the template carries the tag, the `Rc`
            // goes at the payload offset.
            put(b, l.profile_body, &l.profile_body_template);
            let body = contribution::profile_body(item);
            put_words(b, l.profile_body + l.profile_body_payload, &body);
        }
        vec_of(&l.vec, &bytes, sections.len())
    }
}

impl ModalProvider for Rekindle {
    fn modals(&self) -> [usize; 3] {
        let Some(l) = layout() else { return [0; 3] };
        let Some(m) = contribution::modal() else {
            return empty(&l.vec);
        };
        let mut b = vec![0u8; l.modal_size];
        put_words(&mut b, l.modal_key, &string(&m.key));
        put_words(&mut b, l.modal_title, &string(&m.title));
        // Some(String): the String's own bytes (None is a capacity niche).
        let caption = format!("from the {} plugin", m.plugin);
        put_words(&mut b, l.modal_caption, &string(&caption));
        put_words(&mut b, l.modal_body, &contribution::modal_body(&m));
        vec_of(&l.vec, &b, 1)
    }

    fn dismiss(&self, key: &str) {
        queue_click(Click::Modal {
            plugin: key.to_string(),
        });
    }
}

/// rekindle's (zero-sized) provider boxed as the trait object `T`: the
/// (data, vtable) words of the client's `Box<dyn Trait>`.
pub(super) fn boxed<T: ?Sized>(b: Box<T>) -> [usize; 2] {
    let raw: *mut T = Box::into_raw(b);
    assert_eq!(size_of::<*mut T>(), 16, "a trait object pointer");
    // SAFETY: a `*mut dyn Trait` is two words, data then vtable (its size is
    // asserted above), the representation the client's `Box<dyn Trait>` has
    // (same compiler).
    unsafe { std::mem::transmute_copy(&raw) }
}

impl MessageDecorator for Rekindle {
    fn decorate(
        &self,
        model: *const u8,
        guild: Option<u64>,
        channel: u64,
        message: *const u8,
        grouped: bool,
        look: *mut u8,
    ) {
        crate::client::saw_model(model);
        let Some(l) = layout() else { return };
        let Some(c) = crate::client::Client::at(model) else {
            return;
        };
        let Some(m) = c.message_at(message) else {
            return;
        };
        let author_roles = guild
            .and_then(|g| c.member(g, m.author))
            .map(|mb| mb.roles)
            .unwrap_or_default();
        let author = c.user(m.author);
        let req = crate::render::RenderRequest {
            guild,
            channel,
            message: m,
            author,
            author_roles,
            grouped,
        };
        if let Some(found) = crate::render::look_for(req) {
            // SAFETY: the client's own `MessageLook`, handed to its
            // decorators to change, with the fields the layout names
            // (`RegistryLayout::check`).
            unsafe { write_look(&l, look.addr(), &found) };
            crate::render::note_written(&found);
        }
    }
}

/// Take the client's `String` at `at`, leaving an empty one there.
///
/// # Safety
/// `at` is a live `String` of the client's that rekindle may change.
unsafe fn take_string(at: usize) -> String {
    // SAFETY: the caller's contract; a `String` is three words, the
    // representation rekindle's has (same compiler and allocator).
    unsafe {
        let words = rekindle_native::mem::read::<[usize; 3]>(at);
        rekindle_native::mem::write::<[usize; 3]>(at, string(""));
        std::mem::transmute::<[usize; 3], String>(words)
    }
}

/// Add rekindle's look to the client's `MessageLook` at `at`, after what
/// the client's own decorators set.
///
/// # Safety
/// `at` is the client's `MessageLook`, being decorated, laid out as `l`
/// says.
unsafe fn write_look(l: &super::RegistryLayout, at: usize, look: &crate::render::Look) {
    use rekindle_native::mem;
    let words = [l.vec.cap_off, l.vec.ptr_off, l.vec.len_off];
    for b in &look.badges {
        let mut e = vec![0u8; l.badge_size];
        put_words(&mut e, l.badge_text, &string(&b.text));
        put_words(&mut e, l.badge_tooltip, &string(&b.tooltip));
        put(&mut e, l.badge_color, &b.color.to_ne_bytes());
        // SAFETY: the look's badges `Vec`, of `Badge`s this size.
        let _ = unsafe { mem::push_vec(at + l.look_badges, words, &e) };
    }
    for (start, end, color) in &look.highlights {
        let mut e = vec![0u8; l.highlight_size];
        put(&mut e, l.highlight_start, &start.to_ne_bytes());
        put(&mut e, l.highlight_end, &end.to_ne_bytes());
        put(&mut e, l.highlight_color, &color.to_ne_bytes());
        // SAFETY: the look's highlights `Vec`, of `Highlight`s this size.
        let _ = unsafe { mem::push_vec(at + l.look_highlights, words, &e) };
    }
    // SAFETY: fields of the caller's `MessageLook`, at the layout's offsets.
    unsafe {
        if look.name_color != 0 {
            mem::write::<u32>(at + l.look_name_color, look.name_color);
        }
        if look.tint != 0 {
            mem::write::<u32>(at + l.look_tint, look.tint);
        }
        if look.blur {
            mem::write::<u8>(at + l.look_blur, 1);
        }
        for (field, text) in [
            (l.look_append, &look.append),
            (l.look_replace, &look.replace),
            (l.look_collapse, &look.collapse),
        ] {
            if text.is_empty() {
                continue;
            }
            let mut s = take_string(at + field);
            if field == l.look_append && !s.is_empty() {
                s.push(' ');
                s.push_str(text);
            } else {
                s.clone_from(text);
            }
            mem::write::<[usize; 3]>(at + field, std::mem::transmute::<String, [usize; 3]>(s));
        }
    }
    for name in &look.by {
        let e = string(name);
        let mut bytes = [0u8; 24];
        put_words(&mut bytes, 0, &e);
        // SAFETY: the look's `by` `Vec<String>`.
        let _ = unsafe { mem::push_vec(at + l.look_by, words, &bytes) };
    }
}
