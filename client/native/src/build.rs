//! Building elements: `div()` and text through the client's own gpui
//! functions, styled and wired by writing their bytes at recovered offsets.
//!
//! A `Div` under construction is a buffer of rekindle's ([`Bytes`]) until
//! `into_any_element` copies it into gpui's element arena. gpui then owns
//! what it holds (its style boxes, listeners and children) and frees them
//! with the frame, so each of those is allocated with this DLL's global
//! allocator, the process heap the client frees to.

use std::collections::BTreeSet;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, Location, catch_unwind};
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};

use crate::bytes::{Bytes, put, put_word, word};
use crate::client::{self, Ctx, Focus};
use crate::layout::{Resolved, Tags};
use crate::style::{Align, Cursor, Dir, Handler, KeyHandler, Len, Node, Style};
use crate::{AnyElement, fail, mem, paint, scroll};

/// An `AnyElement`'s size: three words.
const ELEMENT: usize = std::mem::size_of::<AnyElement>();

/// Build `node` as an element of the client's gpui.
///
/// # Safety
/// On the client's UI thread, while it draws a frame (the element arena is
/// cleared after each), with `r` the layout `crate::configure` resolved:
/// its functions are the client's own, with the signatures `crate::Fns`
/// gives them.
pub(crate) unsafe fn build(r: &Arc<Resolved>, node: &Node) -> Result<AnyElement> {
    let d = match node {
        // SAFETY: the caller's contract.
        Node::Text(s) => return Ok(unsafe { text(r, intern(s)) }),
        Node::Div(d) => d,
    };
    // Children first: if one fails, no Div is left half-built.
    let kids = d
        .children
        .iter()
        // SAFETY: the caller's contract.
        .map(|c| unsafe { build(r, c) })
        .collect::<Result<Vec<_>>>()?;
    // SAFETY: the caller's contract.
    let mut div = unsafe { new_div(r)? };
    // SAFETY: `new_div` checked the `InteractivityBox` (or the inline
    // `Interactivity`) is there, rekindle's until `into_any`, so nothing
    // else touches it meanwhile.
    let inter = unsafe { interactivity(r, &mut div) };
    // SAFETY: `div()` left the Div's `Box<StyleRefinement>` at `base_style`
    // (`new_div` checked it's there), `style_size` bytes long.
    let style = unsafe {
        std::slice::from_raw_parts_mut(word(inter, r.base_style) as *mut u8, r.style_size)
    };
    // Extra styles start from a copy of the still-default base.
    let default = style.to_vec();
    write_style(r, style, &d.style);
    if let Some(c) = d.style.hover_background {
        let hover = Style {
            background: Some(c),
            ..Style::default()
        };
        put_word(
            &mut *inter,
            r.i.hover_style,
            boxed(&styled(r, &default, &hover)),
        );
    }
    if let Some(key) = &d.style.scroll_y {
        scroll::attach(r, &mut *inter, style, key);
    }
    if d.style.occlude {
        inter[r.i.hitbox_behavior] = r.t.hitbox_block_mouse;
    }
    if let Some(h) = &d.on_click {
        let listener = mouse_down_listener(r, h.clone());
        push_listener(r, &mut *inter, r.i.mouse_down_listeners, listener);
    }
    if let Some(focus) = &d.focus {
        // SAFETY: the caller's contract.
        unsafe { track_focus(r, &mut *inter, focus) };
    }
    if let Some(h) = &d.on_key {
        let listener = key_down_listener(r, h.clone());
        push_listener(r, &mut *inter, r.i.key_down_listeners, listener);
    }
    if let Some(name) = &d.group {
        write_static_str(r, &mut inter[r.i.group..], intern(name));
    } else if d.paint.is_some() {
        // A group gets a hitbox, which is where the painted element finds
        // its bounds.
        write_static_str(r, &mut inter[r.i.group..], "rekindle-paint");
    }
    if let Some((name, style)) = &d.group_hover {
        let at = r.i.group_hover_style;
        write_static_str(r, &mut inter[at..], intern(name));
        let boxed = boxed(&styled(r, &default, style));
        put_word(&mut *inter, at + r.i.group_style_box, boxed);
    }
    for k in kids {
        push_child(r, &mut div, k);
    }
    // SAFETY: the caller's contract.
    let mut e = unsafe { into_any(r, div) };
    if let (Some(items), Some(Len::Px(w)), Some(Len::Px(h))) =
        (&d.paint, d.style.width, d.style.height)
    {
        // SAFETY: `e` is the element gpui just made of a div, and the
        // caller's contract puts this on the UI thread.
        unsafe { paint::attach(r, &mut e, items.clone(), (w, h)) };
    }
    Ok(e)
}

/// `div()`: the client's own `div()` (or, on a build without it,
/// `Interactivity::new` into a zeroed Div; see `crate::Fns`).
///
/// # Safety
/// As [`build`].
pub(crate) unsafe fn new_div(r: &Resolved) -> Result<Bytes> {
    let mut div = Bytes::zeroed(r.div_size);
    if let Some(b) = &r.div_boxed {
        // A build whose Div boxes the Interactivity: no constructor to
        // call, so make one from the measured default (and a fresh style
        // box), then point the Div's box at it. `mem::alloc` is the process
        // heap the client frees to, as for the style boxes below.
        let style = mem::alloc(r.style_size, 8, false);
        let inter = mem::alloc(b.size, 8, false);
        // SAFETY: both are fresh allocations of at least the template's
        // length (`resolve` checked the Interactivity default is `b.size`
        // bytes and the style default is `style_size`).
        unsafe {
            std::ptr::copy_nonoverlapping(b.style_default.as_ptr(), style, r.style_size);
            std::ptr::copy_nonoverlapping(b.default.as_ptr(), inter, b.size);
            mem::write(inter.add(r.base_style) as usize, style as usize);
        }
        put_word(&mut div, b.offset, b.heap_tag);
        put_word(&mut div, b.offset + b.ptr, inter as usize);
    } else if r.fns.div_new != 0 {
        // SAFETY: the caller's contract: the client's `div()`, writing a
        // whole Div into `div`.
        unsafe {
            let f = std::mem::transmute::<usize, unsafe fn(*mut u8)>(r.fns.div_new);
            f(div.as_mut_ptr());
        }
    } else {
        // SAFETY: the caller's contract: this is the client's
        // `Interactivity::new`, writing a Div's first bytes into `div`,
        // which holds a whole Div.
        unsafe {
            let f =
                std::mem::transmute::<usize, unsafe fn(*mut u8, *const Location<'static>) -> *mut u8>(
                    r.fns.interactivity_new,
                );
            f(div.as_mut_ptr(), Location::caller());
        }
    }
    // SAFETY: `div` is the Div the client just wrote.
    let inter = unsafe { interactivity(r, &mut div) };
    let style = word(inter, r.base_style);
    if style == 0 || !style.is_multiple_of(8) {
        bail!("div() left no style box at +{}", r.base_style);
    }
    Ok(div)
}

/// The Div's `Interactivity`, as bytes: the Div itself where the build keeps
/// it inline, or the allocation its `InteractivityBox` points at.
///
/// # Safety
/// `div` is a whole Div the client just made, rekindle's until `into_any`,
/// on the thread that draws it.
pub(crate) unsafe fn interactivity<'a>(r: &Resolved, div: &'a mut Bytes) -> &'a mut [u8] {
    match &r.div_boxed {
        None => &mut div[..],
        Some(b) => {
            let ptr = word(div, b.offset + b.ptr);
            assert!(
                ptr != 0 && ptr.is_multiple_of(8),
                "div() left no Interactivity at +{}",
                b.offset + b.ptr
            );
            // SAFETY: the caller's contract: the box points at the
            // `Interactivity` the Div owns, `b.size` bytes, alive until the
            // Div is consumed.
            unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, b.size) }
        }
    }
}

/// The Div as an `AnyElement`. gpui copies its bytes into the element
/// arena and owns what it holds; `div` is then stale bytes, freed without
/// dropping anything.
///
/// # Safety
/// As [`build`], with `div` from [`new_div`].
pub(crate) unsafe fn into_any(r: &Resolved, div: Bytes) -> AnyElement {
    let mut out: AnyElement = [0; 3];
    // SAFETY: the caller's contract: the client's `<Div as
    // IntoElement>::into_any_element`, given a whole Div `div()` made.
    unsafe {
        let f = std::mem::transmute::<
            usize,
            unsafe fn(*mut AnyElement, *const u8) -> *mut AnyElement,
        >(r.fns.div_into_any_element);
        f(&raw mut out, div.as_ptr());
    }
    drop(div);
    out
}

/// `s` as a text element.
///
/// # Safety
/// As [`build`].
unsafe fn text(r: &Resolved, s: &'static str) -> AnyElement {
    let mut out: AnyElement = [0; 3];
    // SAFETY: the caller's contract: the client's `<&str as
    // IntoElement>::into_any_element`, given a string that lives for the
    // process, as `&'static str` promises gpui.
    unsafe {
        let f = std::mem::transmute::<
            usize,
            unsafe fn(*mut AnyElement, *const u8, usize) -> *mut AnyElement,
        >(r.fns.str_into_any_element);
        f(&raw mut out, s.as_ptr(), s.len());
    }
    out
}

/// `.child(e)`: push `e` onto the Div's children, a `SmallVec<[AnyElement;
/// 2]>` (smallvec 1.x: inline storage, or (ptr, len) once spilled, then the
/// capacity, which is the length while inline).
pub(crate) fn push_child(r: &Resolved, div: &mut Bytes, e: AnyElement) {
    let at = r.children;
    let cap_at = at + r.c.children_capacity;
    let inline = r.c.children_inline;
    let cap = word(div, cap_at);
    if cap < inline {
        // Inline, with room: `cap` is the length.
        for (k, w) in e.iter().enumerate() {
            put_word(div, at + cap * ELEMENT + k * 8, *w);
        }
        put_word(div, cap_at, cap + 1);
        return;
    }
    let mut heap: Vec<AnyElement> = if cap == inline {
        // Inline and full: move to the heap.
        let mut v = Vec::with_capacity(inline * 2);
        v.extend((0..inline).map(|i| {
            let e = at + i * ELEMENT;
            [word(div, e), word(div, e + 8), word(div, e + 16)]
        }));
        v
    } else {
        // SAFETY: this Div is rekindle's and only this function pushes its
        // children, so a spilled buffer is the `Vec<AnyElement>` the branch
        // above or a previous push made: `len` elements, capacity `cap`.
        unsafe { Vec::from_raw_parts(word(div, at) as *mut AnyElement, word(div, at + 8), cap) }
    };
    heap.push(e);
    // The Div owns the buffer now, and gpui frees it with the Div.
    let mut heap = ManuallyDrop::new(heap);
    put_word(div, at, heap.as_mut_ptr() as usize);
    put_word(div, at + 8, heap.len());
    put_word(div, cap_at, heap.capacity());
}

/// `&'static str` for a text element: each distinct string is leaked once.
pub(crate) fn intern(s: &str) -> &'static str {
    static STRINGS: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut strings = STRINGS.lock().unwrap();
    if let Some(v) = strings.get(s) {
        return v;
    }
    let leaked: &'static str = Box::leak(s.into());
    strings.insert(leaked);
    leaked
}

/// A copy of the default style `default` with `style` written over it.
fn styled(r: &Resolved, default: &[u8], style: &Style) -> Vec<u8> {
    let mut b = default.to_vec();
    write_style(r, &mut b, style);
    b
}

/// A `Box<StyleRefinement>` of `bytes` that the client will own and free:
/// its address.
fn boxed(bytes: &[u8]) -> usize {
    let p = mem::alloc(bytes.len(), 8, false);
    // SAFETY: `p` is a fresh allocation of `bytes.len()` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
    p as usize
}

/// A `SharedString` (`SmolStr`) of a `&'static str`, in its static form, at
/// the start of `at`.
fn write_static_str(r: &Resolved, at: &mut [u8], s: &'static str) {
    at[..24].fill(0);
    at[0] = r.t.shared_string_static;
    put_word(at, r.i.shared_string_ptr, s.as_ptr() as usize);
    put_word(at, r.i.shared_string_len, s.len());
}

/// What gpui calls for a mouse-down on the element: `Fn(&MouseDownEvent,
/// DispatchPhase, &Hitbox, &mut Window, &mut App)`. A closure of this crate's
/// with the same argument shapes has the same `dyn Fn` vtable layout and call
/// ABI (same compiler), so gpui can call and drop it.
type MouseDown = dyn Fn(*const u8, u8, *const u8, *mut u8, *mut u8);

/// What gpui calls for a key-down on the focus path: `Fn(&KeyDownEvent,
/// DispatchPhase, &mut Window, &mut App)`.
type KeyDown = dyn Fn(*const u8, u8, *mut u8, *mut u8);

/// `.on_mouse_down(Left, ..)`, as gpui's own wrapper does it: bubble phase,
/// left button, and the element's hitbox hovered (gpui's `Hitbox::is_hovered`,
/// so occlusion counts), then the handler. As (data, vtable).
fn mouse_down_listener(r: &Arc<Resolved>, h: Handler) -> [usize; 2] {
    let r = Arc::clone(r);
    let listener: Box<MouseDown> = Box::new(move |event, phase, hitbox, window, cx| {
        // SAFETY: gpui calls this with a live `&MouseDownEvent`, and
        // `mouse_button` is its `button` byte.
        let button = unsafe { mem::read::<u8>(event.addr() + r.c.mouse_button) };
        if phase != r.t.phase_bubble || button != r.c.button_left {
            return;
        }
        // SAFETY: gpui passes the element's live hitbox and the window it's in.
        if !unsafe { client::hitbox_hovered(&r.c, hitbox, window) } {
            return;
        }
        let mut ctx = Ctx { window, cx };
        // SAFETY: as above: a live hitbox.
        unsafe { client::note_pressed(&r.c, hitbox) };
        let panicked = catch_unwind(AssertUnwindSafe(|| (h.0)(&mut ctx))).is_err();
        // SAFETY: null clears the bounds.
        unsafe { client::note_pressed(&r.c, std::ptr::null()) };
        if panicked {
            fail("panic in a click handler");
        }
    });
    // SAFETY: a `*mut dyn Fn` is two words, data then vtable, the
    // representation the client's `Box<dyn Fn>` has (same compiler).
    unsafe { std::mem::transmute::<*mut MouseDown, [usize; 2]>(Box::into_raw(listener)) }
}

/// `.on_key_down(..)`, as gpui's wrapper does it: bubble phase only. As
/// (data, vtable).
fn key_down_listener(r: &Arc<Resolved>, h: KeyHandler) -> [usize; 2] {
    let r = Arc::clone(r);
    let listener: Box<KeyDown> = Box::new(move |event, phase, window, cx| {
        if phase != r.t.phase_bubble {
            return;
        }
        // SAFETY: gpui calls this with a live `&KeyDownEvent`, whose
        // keystroke is at its start.
        let key = unsafe { client::read_key(&r.c, event.addr()) };
        let mut ctx = Ctx { window, cx };
        if catch_unwind(AssertUnwindSafe(|| (h.0)(&mut ctx, &key))).is_err() {
            fail("panic in a key handler");
        }
    });
    // SAFETY: as in `mouse_down_listener`.
    unsafe { std::mem::transmute::<*mut KeyDown, [usize; 2]>(Box::into_raw(listener)) }
}

/// Make the fresh (empty) listener `Vec` at `at` in the Div hold `listener`
/// alone. A one-element `Vec`'s buffer is the allocation `Box` makes for
/// that element.
fn push_listener(r: &Resolved, div: &mut [u8], at: usize, listener: [usize; 2]) {
    let buf = Box::into_raw(Box::new(listener));
    let [cap, ptr, len] = r.c.vec;
    put_word(div, at + cap, 1);
    put_word(div, at + ptr, buf as usize);
    put_word(div, at + len, 1);
}

/// `.track_focus(&focus)`: `focusable`, and a clone (the client's own
/// `FocusHandle::clone`, so the focus map's count is right when gpui drops it).
///
/// # Safety
/// As [`build`].
unsafe fn track_focus(r: &Resolved, div: &mut [u8], focus: &Focus) {
    let slot = div[r.i.tracked_focus_handle..].as_mut_ptr();
    // SAFETY: the caller's contract: the client's `FocusHandle::clone`,
    // writing a handle into the Div's `Option<FocusHandle>` (32 bytes,
    // inside the Div: checked by `resolve`) from one rekindle holds.
    unsafe {
        let clone = std::mem::transmute::<usize, unsafe fn(*mut u8, *const u8) -> *mut u8>(
            r.fns.focus_handle_clone,
        );
        clone(slot, focus.as_ptr());
    }
    div[r.i.focusable] = 1;
}

/// Writes a `StyleRefinement`'s fields, each tagged as gpui encodes it.
struct StyleWriter<'a> {
    b: &'a mut [u8],
    t: &'a Tags,
}

impl StyleWriter<'_> {
    fn bytes(&mut self, at: usize, bytes: &[u8]) {
        put(self.b, at, bytes);
    }

    fn byte(&mut self, at: usize, v: u8) {
        self.b[at] = v;
    }

    fn u32(&mut self, at: usize, v: u32) {
        put(self.b, at, &v.to_ne_bytes());
    }

    fn f32(&mut self, at: usize, v: f32) {
        put(self.b, at, &v.to_ne_bytes());
    }

    /// Four f32s, h, s, l, a.
    fn hsla(&mut self, at: usize, c: [f32; 4]) {
        for (i, v) in c.into_iter().enumerate() {
            self.f32(at + 4 * i, v);
        }
    }

    /// `Some(Hsla)`: a u32 tag, the colour at +4.
    fn some_hsla(&mut self, at: usize, c: [f32; 4]) {
        self.u32(at, self.t.option_some);
        self.hsla(at + 4, c);
    }

    /// `Some` length in pixels: a u32 tag, the f32 at +4.
    fn px(&mut self, at: usize, v: f32) {
        self.u32(at, self.t.len_pixels);
        self.f32(at + 4, v);
    }

    fn len(&mut self, at: usize, l: Len) {
        let (tag, v) = match l {
            Len::Px(v) => (self.t.len_pixels, v),
            Len::Frac(v) => (self.t.len_fraction, v),
            Len::Auto => (self.t.len_auto, 0.0),
        };
        self.u32(at, tag);
        self.f32(at + 4, v);
    }

    fn align(&self, a: Align) -> u8 {
        match a {
            Align::Start => self.t.align_start,
            Align::Center => self.t.align_center,
            Align::End => self.t.align_end,
        }
    }
}

/// Write `style` into `b`, a default (all-`None`) `StyleRefinement`.
/// `resolve` checked every field written lies inside it.
fn write_style(r: &Resolved, b: &mut [u8], style: &Style) {
    let (s, t) = (&r.s, &r.t);
    let mut w = StyleWriter { b, t };
    if let Some(c) = style.text_color {
        w.some_hsla(s.text_color, c.to_hsla());
    }
    if let Some(v) = style.text_size {
        w.px(s.text_size, v);
    }
    if let Some(v) = style.line_height {
        w.px(s.text_line_height, v);
    }
    if let Some(c) = style.background {
        // `Some(Fill)` as gpui builds it, with this colour.
        w.bytes(s.background, &r.fill);
        w.hsla(s.background + r.c.fill_hsla, c.to_hsla());
    }
    if let Some((width, c)) = style.border {
        for at in s.border_widths {
            w.px(at, width);
        }
        w.some_hsla(s.border_color, c.to_hsla());
    }
    let p = style.padding;
    for (at, v) in s
        .padding
        .into_iter()
        .zip([p.top, p.right, p.bottom, p.left])
    {
        if let Some(v) = v {
            w.px(at, v);
        }
    }
    let i = style.inset;
    for (at, v) in s.inset.into_iter().zip([i.top, i.right, i.bottom, i.left]) {
        if let Some(v) = v {
            w.len(at, v);
        }
    }
    if let Some(v) = style.radius {
        for at in s.corners {
            w.px(at, v);
        }
    }
    if let Some(v) = style.width {
        w.len(s.width, v);
    }
    if let Some(v) = style.height {
        w.len(s.height, v);
    }
    if let Some(v) = style.gap {
        // SizeRefinement<DefiniteLength>: width then height.
        w.px(s.gap, v);
        w.px(s.gap + 8, v);
    }
    if style.absolute {
        w.byte(s.position, t.position_absolute);
    }
    if let Some(weight) = style.font_weight {
        w.u32(s.font_weight, t.font_weight_some);
        w.f32(s.font_weight + 4, weight);
    }
    if let Some(c) = style.cursor {
        let tag = match c {
            Cursor::Pointer => t.cursor_pointer,
            Cursor::Text => t.cursor_text,
        };
        w.byte(s.mouse_cursor, tag);
    }
    if let Some(v) = style.visible {
        let tag = if v {
            t.visibility_visible
        } else {
            t.visibility_hidden
        };
        w.byte(s.visibility, tag);
    }
    if let Some(d) = style.flex {
        w.byte(s.display, t.display_flex);
        let tag = match d {
            Dir::Row => t.flex_row,
            Dir::Column => t.flex_column,
        };
        w.byte(s.flex_direction, tag);
    }
    if let Some(a) = style.align_items {
        let tag = w.align(a);
        w.byte(s.align_items, tag);
    }
    if let Some(a) = style.justify {
        let tag = w.align(a);
        w.byte(s.justify_content, tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::tests::mock_spec;
    use crate::style::{Edges, Rgba};

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "the values written are read back unchanged"
    )]
    fn writes_land_where_gpui_puts_them() {
        let r = mock_spec().resolve().unwrap();
        let mut style_box = vec![0u8; r.style_size];
        let st = Style {
            background: Some(Rgba::hex(0xff0000ff)),
            text_color: Some(Rgba::hex(0xffffffff)),
            padding: Edges::xy(Some(10.0), Some(4.0)),
            width: Some(Len::Frac(1.0)),
            inset: Edges {
                top: Some(Len::Px(8.0)),
                right: None,
                bottom: None,
                left: Some(Len::Auto),
            },
            absolute: true,
            flex: Some(Dir::Column),
            align_items: Some(Align::Center),
            border: Some((1.0, Rgba::hex(0x00ff00ff))),
            ..Default::default()
        };
        write_style(&r, &mut style_box, &st);
        let f32_at = |o: usize| f32::from_le_bytes(style_box[o..o + 4].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(style_box[o..o + 4].try_into().unwrap());
        // Border: every side 1px (AbsoluteLength pixels), colour Some(green).
        for o in [272, 280, 288, 296] {
            assert_eq!((u32_at(o), f32_at(o + 4)), (0, 1.0));
        }
        assert_eq!(u32_at(592), 1);
        // Fill: the template's tag bytes, then hsla(0, 1, .5, 1) at +8.
        assert_eq!(&style_box[336..344], &r.fill[..8]);
        assert_eq!(
            [f32_at(344), f32_at(348), f32_at(352), f32_at(356)],
            [0.0, 1.0, 0.5, 1.0]
        );
        // Option<Hsla>: tag 1, then white.
        assert_eq!(u32_at(136), 1);
        assert_eq!(f32_at(136 + 12), 1.0);
        // Padding: top 4 (pixels), right 10.
        assert_eq!((u32_at(408), f32_at(412)), (0, 4.0));
        assert_eq!((u32_at(416), f32_at(420)), (0, 10.0));
        // Width 100%: Fraction (2), 1.0.
        assert_eq!((u32_at(488), f32_at(492)), (2, 1.0));
        // Inset: top 8px, right untouched (None stays as default), left auto.
        assert_eq!((u32_at(456), f32_at(460)), (0, 8.0));
        assert_eq!(u32_at(464), 0, "untouched bytes stay as given");
        assert_eq!(u32_at(480), 0xffff_ffff);
        assert_eq!(style_box[629], 1, "absolute");
        assert_eq!((style_box[633], style_box[634]), (1, 1), "flex column");
        assert_eq!(style_box[635], 4, "align center");
    }

    #[test]
    fn children_go_inline_then_to_the_heap() {
        let r = mock_spec().resolve().unwrap();
        let mut div = Bytes::zeroed(r.div_size);
        let inline = r.c.children_inline;
        let cap_at = r.children + r.c.children_capacity;
        let kids: Vec<AnyElement> = (0..5).map(|i| [i, i + 100, i + 200]).collect();
        for (n, k) in kids.iter().enumerate() {
            push_child(&r, &mut div, *k);
            if n < inline {
                assert_eq!(
                    word(&div, cap_at),
                    n + 1,
                    "inline: the capacity is the length"
                );
            }
        }
        let (ptr, len, cap) = (
            word(&div, r.children),
            word(&div, r.children + 8),
            word(&div, cap_at),
        );
        assert_eq!((len, cap), (5, 8));
        // SAFETY: the buffer `push_child` made, handed back to drop it.
        let heap = unsafe { Vec::from_raw_parts(ptr as *mut AnyElement, len, cap) };
        assert_eq!(heap, kids);
    }

    #[test]
    fn interned_strings_are_stable() {
        let a = intern("hello");
        let b = intern("hello");
        assert_eq!(a.as_ptr(), b.as_ptr());
    }
}
