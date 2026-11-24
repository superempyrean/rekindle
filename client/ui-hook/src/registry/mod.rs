//! rekindle as one more provider in each of the client's own registries.
//!
//! The client builds its registries once, in `Registries::with_builtins`, by
//! registering its own providers (trait objects it calls) and entries (data it
//! reads): switcher, chat bar, context menus, profile popout, modals, settings
//! sections, overlay layers. rekindle hooks that one function and, after the
//! client's own registrations, adds its own the same way. From then on the
//! client calls rekindle exactly as it calls its own providers. Its own
//! code lays out the rows, draws the buttons and tooltips, shows the modal
//! with its scrim, close button and Esc, lists the settings page in its
//! sidebar, and routes every interaction to rekindle by provider index. No
//! per-method detours, so nothing an optimizer can inline away from under
//! rekindle.
//!
//! Before writing beside them, rekindle checks the client's own entries
//! look like the recovered layout says ([`crate::provenance`]). A registry
//! whose entries don't is left to the client.

mod contribution;
mod provider;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use rekindle_native::mem;

pub use contribution::{
    Button, Command, MenuItem, MenuKind, MenuTargetRef, Modal, ProfileSectionItem, menu_rows,
    modal_panel, modal_plugin, set_buttons, set_commands, set_menu_items, set_modal, set_profiles,
};
pub(crate) use contribution::{
    MENU_INVOKES, MODAL_RENDERS, PROFILE_OPENS, PROFILE_RENDERS, buttons, modal_title,
};
pub use provider::{ACTION, ButtonAction};
use provider::{
    ChatBarProvider, ContextMenuProvider, MessageDecorator, ModalProvider, ProfileSectionProvider,
    Rekindle, SwitcherProvider, boxed, put, put_words, string,
};

use crate::VecLayout;

/// The registries rekindle joins. One whose layout this build doesn't
/// confirm is left to the client: rekindle registers nothing there, and the
/// slots it backs report `unavailable`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Surfaces {
    pub switcher: bool,
    pub chat_bar: bool,
    pub menus: bool,
    pub profiles: bool,
    pub modals: bool,
    pub settings: bool,
    pub overlays: bool,
    pub decorators: bool,
}

/// Where everything the providers build sits. All from the recovered layout.
/// The fields of a registry that's off in `surfaces` are never read.
#[derive(Clone, Debug, Default)]
pub struct RegistryLayout {
    pub surfaces: Surfaces,
    /// `Registries::with_builtins(ret) -> ret`.
    pub with_builtins_addr: usize,
    /// Field offsets in `Registries`.
    pub settings: usize,
    pub switcher: usize,
    pub chat_bar: usize,
    pub menus: usize,
    pub profiles: usize,
    pub modals: usize,
    pub overlays: usize,
    /// `ChatBar.providers` (its `buttons` Vec is the other field).
    pub chat_bar_providers: usize,
    /// `SwitcherRow`: size, and its four `String`s.
    pub row_size: usize,
    pub row_key: usize,
    pub row_glyph: usize,
    pub row_title: usize,
    pub row_detail: usize,
    /// `ChatBarButton`.
    pub button_size: usize,
    pub button_id: usize,
    pub button_label: usize,
    pub button_tooltip: usize,
    pub button_order: usize,
    pub button_action: usize,
    /// `MenuRow`, and the `MenuTarget` it's opened on.
    pub menu_row_size: usize,
    pub menu_row_key: usize,
    pub menu_row_label: usize,
    pub menu_row_danger: usize,
    pub target_tag_message: u64,
    pub target_tag_user: u64,
    pub target_tag_channel: u64,
    pub target_id: usize,
    pub target_message_channel: usize,
    pub target_message_author: usize,
    /// `ProfileSection`: id, title, and the body (`ProfileBody::Render`: the
    /// template carries the tag, the `Rc` goes at the payload offset).
    pub profile_size: usize,
    pub profile_id: usize,
    pub profile_title: usize,
    pub profile_body: usize,
    pub profile_body_template: Vec<u8>,
    pub profile_body_payload: usize,
    /// `ModalSpec`: key, title, caption (`Option<String>`), body.
    pub modal_size: usize,
    pub modal_key: usize,
    pub modal_title: usize,
    pub modal_caption: usize,
    pub modal_body: usize,
    /// `SettingsSection`, and rekindle's page.
    pub section_size: usize,
    pub section_id: usize,
    pub section_label: usize,
    pub section_group: usize,
    pub section_order: usize,
    pub section_render: usize,
    pub page_id: String,
    pub page_label: String,
    pub page_group: String,
    /// `Overlay`: id and render.
    pub overlay_size: usize,
    pub overlay_id: usize,
    pub overlay_render: usize,
    /// `Registries.decorators`, and its providers `Vec`.
    pub decorators: usize,
    pub decorators_providers: usize,
    /// `MessageLook`: what a decorator changes.
    pub look_size: usize,
    pub look_badges: usize,
    pub look_highlights: usize,
    pub look_name_color: usize,
    pub look_tint: usize,
    pub look_append: usize,
    pub look_replace: usize,
    pub look_collapse: usize,
    pub look_blur: usize,
    pub look_by: usize,
    /// `Badge` and `Highlight`, the entries of its `Vec`s.
    pub badge_size: usize,
    pub badge_text: usize,
    pub badge_tooltip: usize,
    pub badge_color: usize,
    pub highlight_size: usize,
    pub highlight_start: usize,
    pub highlight_end: usize,
    pub highlight_color: usize,
    /// Where a `Vec`'s three words sit (this toolchain's, recovered).
    pub vec: VecLayout,
    /// The client action's size and its "do nothing" encoding (tag).
    pub action_size: usize,
    pub action_none_tag: u64,
}

/// A value of the client's that rekindle writes: whether its registry is
/// on, its name, its size, and the (offset, width) of each field written.
type CheckedValue<'a> = (bool, &'a str, usize, Vec<(usize, usize)>);

impl RegistryLayout {
    /// `Err` unless every field rekindle writes in a value of the client's
    /// lies inside that value, for each registry that is on.
    fn check(&self) -> Result<()> {
        const STRING: usize = 24;
        const FAT: usize = 16;
        let s = &self.surfaces;
        let values: [CheckedValue; 7] = [
            (
                s.switcher,
                "SwitcherRow",
                self.row_size,
                [
                    self.row_key,
                    self.row_glyph,
                    self.row_title,
                    self.row_detail,
                ]
                .map(|o| (o, STRING))
                .to_vec(),
            ),
            (
                s.chat_bar,
                "ChatBarButton",
                self.button_size,
                vec![
                    (self.button_id, STRING),
                    (self.button_label, STRING),
                    (self.button_tooltip, STRING),
                    (self.button_order, 4),
                    (self.button_action, FAT),
                ],
            ),
            (
                s.menus,
                "MenuRow",
                self.menu_row_size,
                vec![
                    (self.menu_row_key, STRING),
                    (self.menu_row_label, STRING),
                    (self.menu_row_danger, 1),
                ],
            ),
            (
                s.profiles,
                "ProfileSection",
                self.profile_size,
                vec![
                    (self.profile_id, STRING),
                    (self.profile_title, STRING),
                    (self.profile_body, self.profile_body_template.len()),
                    (self.profile_body + self.profile_body_payload, FAT),
                ],
            ),
            (
                s.modals,
                "ModalSpec",
                self.modal_size,
                vec![
                    (self.modal_key, STRING),
                    (self.modal_title, STRING),
                    (self.modal_caption, STRING),
                    (self.modal_body, FAT),
                ],
            ),
            (
                s.settings,
                "SettingsSection",
                self.section_size,
                vec![
                    (self.section_id, STRING),
                    (self.section_label, STRING),
                    (self.section_group, STRING),
                    (self.section_order, 4),
                    (self.section_render, FAT),
                ],
            ),
            (
                s.overlays,
                "Overlay",
                self.overlay_size,
                vec![(self.overlay_id, STRING), (self.overlay_render, FAT)],
            ),
        ];
        for (on, what, size, fields) in values.into_iter().chain(self.decorator_values()) {
            if let Some((off, width)) = fields.iter().find(|(off, width)| off + width > size)
                && on
            {
                bail!("{what}: a field at {off} ({width} bytes) runs past its {size} bytes");
            }
        }
        let mut words = [self.vec.cap_off, self.vec.ptr_off, self.vec.len_off];
        words.sort_unstable();
        if words != [0, 8, 16] {
            bail!("a Vec's words aren't at 0, 8 and 16: {:?}", self.vec);
        }
        let acts = s.switcher || s.chat_bar || s.menus;
        if acts && self.action_size != ACTION {
            bail!(
                "the client's action is {} bytes; rekindle's providers return {ACTION}",
                self.action_size
            );
        }
        Ok(())
    }
}

impl RegistryLayout {
    /// The values a decorator writes: the look, and its badges and
    /// highlights.
    fn decorator_values(&self) -> [CheckedValue<'static>; 3] {
        const STRING: usize = 24;
        let s = &self.surfaces;
        [
            (
                s.decorators,
                "MessageLook",
                self.look_size,
                vec![
                    (self.look_badges, STRING),
                    (self.look_highlights, STRING),
                    (self.look_append, STRING),
                    (self.look_replace, STRING),
                    (self.look_collapse, STRING),
                    (self.look_by, STRING),
                    (self.look_name_color, 4),
                    (self.look_tint, 4),
                    (self.look_blur, 1),
                ],
            ),
            (
                s.decorators,
                "Badge",
                self.badge_size,
                vec![
                    (self.badge_text, STRING),
                    (self.badge_tooltip, STRING),
                    (self.badge_color, 4),
                ],
            ),
            (
                s.decorators,
                "Highlight",
                self.highlight_size,
                vec![
                    (self.highlight_start, 8),
                    (self.highlight_end, 8),
                    (self.highlight_color, 4),
                ],
            ),
        ]
    }
}

static LAYOUT: Mutex<Option<RegistryLayout>> = Mutex::new(None);
static TRAMP: AtomicUsize = AtomicUsize::new(0);

fn layout() -> Option<RegistryLayout> {
    LAYOUT.lock().unwrap().clone()
}

/// A registry rekindle registers a provider in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Switcher = 0,
    ChatBar = 1,
    Menus = 2,
    Profiles = 3,
    Modals = 4,
    Decorators = 5,
}

impl Provider {
    fn name(self) -> &'static str {
        match self {
            Provider::Switcher => "switcher",
            Provider::ChatBar => "chat_bar",
            Provider::Menus => "menus",
            Provider::Profiles => "profiles",
            Provider::Modals => "modals",
            Provider::Decorators => "decorators",
        }
    }
}

/// Each registry's index of rekindle's provider (`usize::MAX`: not
/// registered), as the client numbers its providers. For checks.
static INDEX: [AtomicUsize; 6] = [const { AtomicUsize::new(usize::MAX) }; 6];

/// rekindle's provider index in `registry` (once `with_builtins` ran).
pub fn provider_index(registry: Provider) -> Option<usize> {
    let i = INDEX[registry as usize].load(Ordering::SeqCst);
    (i != usize::MAX).then_some(i)
}

/// Registries rekindle stayed out of when the client built them, and why:
/// their own entries didn't look like the recovered layout says.
static REFUSED: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

/// The registries rekindle stayed out of at run time, and why.
pub(crate) fn refused() -> Vec<(&'static str, String)> {
    REFUSED.lock().unwrap().clone()
}

fn refuse(registry: &'static str, why: String) {
    REFUSED.lock().unwrap().push((registry, why));
}

/// Register rekindle in the client's registries whenever it builds them.
/// Fails if the layout would have rekindle write past a value of the
/// client's, or the detour can't be installed.
///
/// # Safety
///
/// `layout.with_builtins_addr` is the client's `Registries::with_builtins`,
/// and the layout's offsets and sizes are that build's.
pub unsafe fn install(layout: RegistryLayout) -> Result<crate::Hook> {
    use rekindle_hook::{Detour, InlineDetour};
    layout.check()?;
    let addr = layout.with_builtins_addr;
    *LAYOUT.lock().unwrap() = Some(layout);
    // SAFETY: the caller's contract: `addr` is `Registries::with_builtins`,
    // and `hooked_with_builtins` takes and returns what it does.
    let d = unsafe { InlineDetour.install(addr, hooked_with_builtins as *const () as usize)? };
    TRAMP.store(d.trampoline(), Ordering::SeqCst);
    Ok(crate::Hook::new(vec![d]))
}

/// `Registries::with_builtins(ret) -> ret`.
///
/// # Safety
/// Only the client calls this, through the detour, in place of
/// `with_builtins`.
unsafe extern "C" fn hooked_with_builtins(ret: *mut u8) -> *mut u8 {
    // SAFETY: the trampoline runs the client's own `with_builtins`, which
    // builds its registries into `ret`.
    unsafe {
        let original = std::mem::transmute::<usize, unsafe extern "C" fn(*mut u8) -> *mut u8>(
            TRAMP.load(Ordering::SeqCst),
        );
        original(ret);
    }
    if let Some(l) = layout() {
        // SAFETY: `ret` holds the client's registries, just built.
        unsafe { register(ret.addr(), &l) };
    }
    ret
}

/// The entries of the client `Vec` at `vec`, each `stride` bytes, after a
/// check of its header: `len <= cap`, and a readable, aligned buffer.
///
/// # Safety
/// `vec` must be a readable `Vec` header of the client's.
unsafe fn entries(v: &VecLayout, vec: usize, stride: usize) -> Result<Vec<usize>, String> {
    // SAFETY: the caller's contract.
    let (cap, ptr, len) = unsafe {
        (
            mem::read::<usize>(vec + v.cap_off),
            mem::read::<usize>(vec + v.ptr_off),
            mem::read::<usize>(vec + v.len_off),
        )
    };
    if len > cap || len > 4096 || (len > 0 && (ptr == 0 || !ptr.is_multiple_of(8))) {
        return Err(format!("a Vec of length {len}, capacity {cap} at {ptr:#x}"));
    }
    if len > 0 && !mem::readable(ptr, len * stride) {
        return Err(format!("a Vec's {len} entries at {ptr:#x} aren't readable"));
    }
    Ok((0..len).map(|i| ptr + i * stride).collect())
}

/// Whether the client's own providers in the registry `Vec` at `vec` are
/// `Box<dyn Trait>`s where the layout says.
///
/// # Safety
/// As [`entries`].
unsafe fn providers_ok(v: &VecLayout, vec: usize) -> Result<(), String> {
    // SAFETY: the caller's contract.
    for e in unsafe { entries(v, vec, 16)? } {
        // SAFETY: `entries` checked each entry's 16 bytes readable.
        let fat = unsafe { mem::read::<[usize; 2]>(e) };
        crate::provenance::check_dyn(fat)?;
    }
    Ok(())
}

/// Whether the client's own entries in the registry `Vec` at `vec` have
/// `String`s at `strings` and `dyn` fat pointers at `fats`.
///
/// # Safety
/// As [`entries`], with entries of `stride` bytes that hold those fields
/// (`RegistryLayout::check`).
unsafe fn entries_ok(
    v: &VecLayout,
    vec: usize,
    stride: usize,
    strings: &[usize],
    fats: &[usize],
) -> Result<(), String> {
    let s = crate::provenance::std_string();
    // SAFETY: the caller's contract.
    for e in unsafe { entries(v, vec, stride)? } {
        for off in strings {
            // SAFETY: `entries` checked the entry readable, and the field
            // lies inside it.
            unsafe { crate::provenance::check_string(e + off, &s)? };
        }
        for off in fats {
            // SAFETY: as above.
            let fat = unsafe { mem::read::<[usize; 2]>(e + off) };
            crate::provenance::check_dyn(fat)?;
        }
    }
    Ok(())
}

/// After the client's own registrations: add rekindle's, to each registry
/// whose own entries match the layout.
///
/// # Safety
/// `r` is the client's `Registries`, just built, on the thread that built
/// them.
unsafe fn register(r: usize, l: &RegistryLayout) {
    let mut on = l.surfaces;
    let provider = |registry: Provider, vec: usize, fat: [usize; 2]| {
        // SAFETY: the caller's contract: `vec` is one of the registries'
        // provider `Vec`s.
        if let Err(why) = unsafe { providers_ok(&l.vec, vec) } {
            return refuse(registry.name(), why);
        }
        let mut elem = [0u8; 16];
        put_words(&mut elem, 0, &fat);
        // SAFETY: as above, a `Vec` of 16-byte boxes, which `providers_ok`
        // checked looks like one; the box is rekindle's, for it to own.
        let index = unsafe { mem::read::<usize>(vec + l.vec.len_off) };
        let words = [l.vec.cap_off, l.vec.ptr_off, l.vec.len_off];
        // SAFETY: as above.
        match unsafe { mem::push_vec(vec, words, &elem) } {
            Ok(()) => INDEX[registry as usize].store(index, Ordering::SeqCst),
            Err(e) => refuse(registry.name(), format!("{e:#}")),
        }
    };
    if on.settings {
        // SAFETY: the caller's contract: the settings entry `Vec`.
        let checked = unsafe {
            entries_ok(
                &l.vec,
                r + l.settings,
                l.section_size,
                &[l.section_id, l.section_label, l.section_group],
                &[l.section_render],
            )
        };
        if let Err(why) = checked {
            refuse("settings", why);
            on.settings = false;
            // rekindle's window stands in for the page it can't add.
            crate::window::set_button(true);
        }
    }
    if on.overlays {
        // SAFETY: the caller's contract: the overlay entry `Vec`.
        let checked = unsafe {
            entries_ok(
                &l.vec,
                r + l.overlays,
                l.overlay_size,
                &[l.overlay_id],
                &[l.overlay_render],
            )
        };
        if let Err(why) = checked {
            refuse("overlays", why);
            on.overlays = false;
        }
    }
    if on.switcher {
        let fat = boxed::<dyn SwitcherProvider>(Box::new(Rekindle));
        provider(Provider::Switcher, r + l.switcher, fat);
    }
    if on.chat_bar {
        let fat = boxed::<dyn ChatBarProvider>(Box::new(Rekindle));
        provider(
            Provider::ChatBar,
            r + l.chat_bar + l.chat_bar_providers,
            fat,
        );
    }
    if on.menus {
        let fat = boxed::<dyn ContextMenuProvider>(Box::new(Rekindle));
        provider(Provider::Menus, r + l.menus, fat);
    }
    if on.profiles {
        let fat = boxed::<dyn ProfileSectionProvider>(Box::new(Rekindle));
        provider(Provider::Profiles, r + l.profiles, fat);
    }
    if on.modals {
        let fat = boxed::<dyn ModalProvider>(Box::new(Rekindle));
        provider(Provider::Modals, r + l.modals, fat);
    }
    if on.decorators {
        let fat = boxed::<dyn MessageDecorator>(Box::new(Rekindle));
        provider(
            Provider::Decorators,
            r + l.decorators + l.decorators_providers,
            fat,
        );
    }
    if on.settings {
        // Settings: rekindle's page, last in the client's sorted list.
        let mut s = vec![0u8; l.section_size];
        put_words(&mut s, l.section_id, &string(&l.page_id));
        put_words(&mut s, l.section_label, &string(&l.page_label));
        put_words(&mut s, l.section_group, &string(&l.page_group));
        put(&mut s, l.section_order, &10_000i32.to_ne_bytes());
        put_words(&mut s, l.section_render, &crate::panel::settings_body());
        // SAFETY: the settings `Vec`, whose entries `entries_ok` checked.
        unsafe { push_entry(r + l.settings, l, &s, "settings") };
    }
    if on.overlays {
        // Overlays: rekindle's layer (sprites, clipboard writes), drawn by
        // the client.
        let mut o = vec![0u8; l.overlay_size];
        put_words(&mut o, l.overlay_id, &string("rekindle"));
        put_words(&mut o, l.overlay_render, &crate::overlay::overlay_layer());
        // SAFETY: the overlay `Vec`, whose entries `entries_ok` checked.
        unsafe { push_entry(r + l.overlays, l, &o, "overlays") };
    }
}

/// Push `entry` onto the registry `Vec` at `vec`, or refuse the registry.
///
/// # Safety
/// `vec` is one of the client's registry `Vec`s, just checked, with entries
/// of `entry.len()` bytes; `entry` is a valid one for it to own.
unsafe fn push_entry(vec: usize, l: &RegistryLayout, entry: &[u8], registry: &'static str) {
    let words = [l.vec.cap_off, l.vec.ptr_off, l.vec.len_off];
    // SAFETY: the caller's contract.
    if let Err(e) = unsafe { mem::push_vec(vec, words, entry) } {
        refuse(registry, format!("{e:#}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> RegistryLayout {
        RegistryLayout {
            surfaces: Surfaces {
                switcher: true,
                ..Surfaces::default()
            },
            row_size: 96,
            row_key: 0,
            row_glyph: 24,
            row_title: 48,
            row_detail: 72,
            vec: VecLayout {
                cap_off: 0,
                ptr_off: 8,
                len_off: 16,
            },
            action_size: ACTION,
            ..RegistryLayout::default()
        }
    }

    #[test]
    fn a_field_past_its_value_is_refused() {
        assert!(layout().check().is_ok());
        let mut l = layout();
        l.row_detail = 80;
        assert!(l.check().unwrap_err().to_string().contains("SwitcherRow"));
        // A registry that's off isn't checked.
        l.surfaces.switcher = false;
        assert!(l.check().is_ok());
    }

    #[test]
    fn a_vec_must_be_three_words() {
        let mut l = layout();
        l.vec.len_off = 24;
        assert!(l.check().is_err());
    }
}
