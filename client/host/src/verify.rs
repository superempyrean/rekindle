//! A run that doesn't stay attached: start the plugin runtime in the
//! client's own registries, and check through the client's own functions
//! that each registry carries the plugins' contributions.
//!
//! The checks call the client's functions from this worker thread. They run
//! only when the client stays off screen with nothing drawing, so they don't
//! race gpui on the UI thread; a live client is driven through its event
//! dispatch instead (`rekindle_ui_hook::drive`). A check whose client
//! function this build inlined away reports `n/a`.

use anyhow::{Result, anyhow};
use rekindle_extension::theme::ThemeProfile;
use rekindle_extension::{Driver, RecoveredProfile};
use rekindle_native::mem;
use rekindle_ui_hook::registry::{ButtonAction, Provider, Surfaces, provider_index};
use rekindle_ui_hook::{ComposeLayout, UiInput, Widget};

use crate::hooks::{Hooks, frame_hook};
use crate::layout::Layout;
use crate::profile::{configure_native, ext_args, registry_layout};
use crate::{hex, live};

/// The theme token whose colour the theme check reads.
const THEME_TOKEN: &str = "background-base-low";
/// The user the profile and menu checks open.
const USER: u64 = 1001;
const TEXT_REPLACE: &str = "dev.rekindle.text-replace";
const COPY_USER_URLS: &str = "dev.rekindle.copy-user-urls";

/// A client function that fills a value through its return pointer.
type Ctor = unsafe extern "C" fn(*mut u8) -> *mut u8;

/// Start rekindle inside the client and, unless the client stays live,
/// check each surface. Returns the report's fields and the driver to keep.
pub(crate) fn drive_and_verify(layout: &Layout, hooks: &Hooks) -> Result<(String, Option<Driver>)> {
    let opt = |name: &str| hooks.addr(name).ok();
    let with_builtins = hooks.addr("registries_with_builtins")?;
    configure_native(layout, hooks)?;
    // SAFETY: the anchors resolved against this process's own image, and
    // nothing of the client's is running gpui yet.
    let theme = unsafe { ThemeReader::new(layout, hooks)? };
    let theme_base = theme.as_ref().map(ThemeReader::read);

    let (registry, surfaces_off) = registry_layout(layout, with_builtins)?;
    let on = registry.surfaces;
    let off_field = if surfaces_off.is_empty() {
        String::new()
    } else {
        format!(" surfaces_off={}", surfaces_off.join(","))
    };
    let compose = match (opt("chatmodel_send"), layout.off("chatmodel_channel")) {
        (Some(send_addr), Ok(off_channel)) => Some(ComposeLayout {
            send_addr,
            off_channel,
        }),
        _ => None,
    };
    let compose_on = compose.is_some();
    let profile = RecoveredProfile {
        theme: ThemeProfile {
            addr: opt("theme"),
            return_size: layout.size("theme_return")?,
            offsets: layout.colors.clone(),
        },
        registry: Some(registry),
        compose,
        framework: Some(rekindle_ui_hook::FrameworkLayout {
            frame_hook: frame_hook(hooks)?.0,
        }),
        surfaces: crate::wire::surfaces(),
    };
    let (mut ext, n_plugins) = ext_args();
    // Plugins' queries read the client. Live, the plugins start once the
    // client is up, so `activate` can read it too.
    let client_reads = match crate::profile::client_layout(layout) {
        Ok(cl) => {
            rekindle_ui_hook::client::configure(cl);
            String::new()
        }
        Err(e) => format!(
            " client_reads_off={}",
            format!("{e:#}").replace([' ', '"'], "_")
        ),
    };
    ext.wait_for_client = client_reads.is_empty() && live::stays();
    let ext_waits = ext.wait_for_client;
    // SAFETY: as above, and its offsets come from the layout checked against
    // this build; rekindle-native is configured.
    let mut driver = unsafe { Driver::start(ext, profile) }.map_err(|e| anyhow!(e))?;
    // Outside the chat bar's registry, buttons go in by its shape.
    if !on.chat_bar {
        driver.use_framework_chat_bar();
    }
    driver.rekindle().poll();
    driver.pump();
    let loaded = if ext_waits {
        "when_client_ready".to_string()
    } else {
        driver.rekindle().infos().len().to_string()
    };
    if live::stays() {
        let mode = live::start(&mut driver, true)?;
        return Ok((
            format!(" plugins_loaded={loaded} mode={mode}{off_field}{client_reads}"),
            Some(driver),
        ));
    }

    // SAFETY: as above; the client stays off screen, so nothing else calls
    // into it while the checks run on this thread.
    let probe = unsafe { Probe::new(layout, hooks, on, with_builtins)? };
    let mut f = format!(" plugins_loaded={loaded}/{n_plugins}");
    checks(
        &mut f,
        &probe,
        &mut driver,
        theme.as_ref().zip(theme_base),
        compose_on,
    )?;
    f.push_str(&off_field);
    Ok((f, None))
}

/// Each check, in order, as report fields. Some use what an earlier one
/// left: the chat-bar button opens the modal the modal check dismisses.
fn checks(
    f: &mut String,
    probe: &Probe,
    driver: &mut Driver,
    theme: Option<(&ThemeReader, [u8; 16])>,
    compose_on: bool,
) -> Result<()> {
    field(f, "registered", probe.registered());
    let (base, themed, changed, owner) = match theme {
        Some((t, base)) => {
            let themed = t.read();
            (
                hex(&base),
                hex(&themed),
                (base != themed).to_string(),
                driver.override_owner(THEME_TOKEN).unwrap_or_default(),
            )
        }
        None => (na(None), na(None), na(None), na(None)),
    };
    field(f, "theme_base", base);
    field(f, "theme_plugin", themed);
    field(f, "theme_changed", changed);
    field(f, "theme_owner", owner);
    field(
        f,
        "switcher_rekindle_rows",
        probe
            .switcher_rows()?
            .map_or_else(|| na(None), |n| n.to_string()),
    );
    let (appeared, routed) = probe.chat_bar_button(driver)?;
    field(f, "button_appeared", na(appeared));
    field(f, "button_routed", na(routed));
    // The chat-bar button check opened the modal; without it there is none.
    let (shown, dismissed) = probe.modal(driver, routed.is_some())?;
    field(f, "modal_shown", na(shown));
    field(f, "modal_dismissed", na(dismissed));
    let (opened, section) = probe.profile(driver)?;
    field(f, "profile_section", na(section));
    field(f, "profile_open_routed", na(opened));
    let (page, listed) = probe.settings()?;
    field(f, "settings_plugins_page", na(page));
    field(f, "settings_listed", na(listed));
    field(f, "overlay_layer", na(probe.overlay_layer()?));
    // Give the overlay plugins a few ticks to draw.
    let until = std::time::Instant::now() + std::time::Duration::from_millis(350);
    while std::time::Instant::now() < until {
        driver.tick();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    field(f, "overlay_sprites", driver.overlay_sprites());

    let loaded_ids: Vec<String> = driver
        .rekindle()
        .infos()
        .into_iter()
        .map(|i| i.id)
        .collect();
    let has = |id: &str| loaded_ids.iter().any(|x| x == id);
    let menu = probe.menus(driver)?;
    let menu_field = |b: Option<bool>| if has(COPY_USER_URLS) { na(b) } else { na(None) };
    field(
        f,
        "menu_rows",
        menu.rows.map_or_else(|| na(None), |n| n.to_string()),
    );
    field(f, "menu_routed", menu_field(menu.routed));
    field(f, "clipboard_copied", menu_field(menu.copied));
    field(f, "toast_shown", menu_field(menu.toasted));
    let (widgets, rewritten) = if has(TEXT_REPLACE) && compose_on {
        probe.compose(driver)?
    } else {
        (na(None), na(None))
    };
    field(f, "settings_widgets", widgets);
    field(f, "compose_rewritten", rewritten);
    field(f, "manager_panel", manager_panel(driver, &loaded_ids));
    field(f, "native_spec", true);
    Ok(())
}

fn field(out: &mut String, key: &str, value: impl std::fmt::Display) {
    crate::field(out, key, value);
}

/// A check's result for the report: `true`, `false`, or `n/a` when it
/// couldn't run.
fn na(b: Option<bool>) -> String {
    b.map_or_else(|| "n/a".into(), |b| b.to_string())
}

/// The client's theme function, called for the default appearance: the
/// colour of [`THEME_TOKEN`], before and under rekindle's detour.
struct ThemeReader {
    theme_fn: unsafe extern "C" fn(*mut u8, *const u8) -> *mut u8,
    appearance: Vec<u8>,
    return_size: usize,
    token_off: usize,
}

impl ThemeReader {
    /// `None` without the theme function or the default appearance.
    ///
    /// # Safety
    ///
    /// The anchors `theme` and `appearance_default` are those functions of
    /// the client in this process.
    unsafe fn new(layout: &Layout, hooks: &Hooks) -> Result<Option<Self>> {
        let token_off = *layout
            .colors
            .get(THEME_TOKEN)
            .ok_or_else(|| anyhow!("no {THEME_TOKEN} offset"))?;
        let (Ok(theme_addr), Ok(default_addr)) =
            (hooks.addr("theme"), hooks.addr("appearance_default"))
        else {
            return Ok(None);
        };
        let return_size = layout.size("theme_return")?.max(token_off + 16);
        let mut appearance = vec![0u8; 128];
        // SAFETY: the caller's contract: the client's appearance constructor,
        // which fills its return value, and its theme function.
        let theme_fn = unsafe {
            std::mem::transmute::<usize, Ctor>(default_addr)(appearance.as_mut_ptr());
            std::mem::transmute::<usize, unsafe extern "C" fn(*mut u8, *const u8) -> *mut u8>(
                theme_addr,
            )
        };
        Ok(Some(Self {
            theme_fn,
            appearance,
            return_size,
            token_off,
        }))
    }

    fn read(&self) -> [u8; 16] {
        let mut b = vec![0u8; self.return_size];
        // SAFETY: `new`'s contract: the client's theme function, which fills
        // a value of `return_size` bytes from the appearance it built.
        unsafe { (self.theme_fn)(b.as_mut_ptr(), self.appearance.as_ptr()) };
        let mut c = [0u8; 16];
        c.copy_from_slice(&b[self.token_off..self.token_off + 16]);
        c
    }
}

/// The client's registries and a chat model, built by the client's own
/// constructors, to call its providers with.
///
/// Every method calls client functions with these values and reads what
/// they return at layout offsets. That holds by [`Probe::new`]'s contract.
struct Probe<'a> {
    layout: &'a Layout,
    hooks: &'a Hooks,
    on: Surfaces,
    vec_ptr: usize,
    vec_len: usize,
    registries: Vec<u8>,
    model: Vec<u8>,
    /// Without a model, the checks that call a provider with one are n/a.
    have_model: bool,
}

/// What the context-menu check found.
struct MenuCheck {
    rows: Option<usize>,
    routed: Option<bool>,
    copied: Option<bool>,
    toasted: Option<bool>,
}

impl<'a> Probe<'a> {
    /// # Safety
    ///
    /// `hooks` and `layout` describe the client in this process, and nothing
    /// else calls into the client while the probe is used.
    unsafe fn new(
        layout: &'a Layout,
        hooks: &'a Hooks,
        on: Surfaces,
        with_builtins: usize,
    ) -> Result<Self> {
        let mut registries = vec![0u8; 512];
        let mut model = vec![0u8; 1024];
        // SAFETY: the caller's contract: the registries' constructor (which
        // rekindle now joins) and the chat model's, each filling its return
        // value.
        let have_model = unsafe {
            std::mem::transmute::<usize, Ctor>(with_builtins)(registries.as_mut_ptr());
            hooks
                .addr("chatmodel_new")
                .ok()
                .map(|a| std::mem::transmute::<usize, Ctor>(a)(model.as_mut_ptr()))
                .is_some()
        };
        Ok(Self {
            layout,
            hooks,
            on,
            vec_ptr: layout.off("gpui_vec_ptr")?,
            vec_len: layout.off("gpui_vec_len")?,
            registries,
            model,
            have_model,
        })
    }

    fn opt(&self, name: &str) -> Option<usize> {
        self.hooks.addr(name).ok()
    }

    /// The registry `name` inside the client's registries.
    fn reg(&self, name: &str) -> Result<*const u8> {
        let off = self.layout.off(name)?;
        anyhow::ensure!(off < self.registries.len(), "{name} is past the registries");
        Ok(self.registries[off..].as_ptr())
    }

    /// The (pointer, length) of the `Vec` or `String` at `at`.
    ///
    /// # Safety
    ///
    /// `at` is a `Vec` or `String` the client built.
    unsafe fn words(&self, at: usize) -> (usize, usize) {
        // SAFETY: the caller's contract.
        unsafe {
            (
                mem::read::<usize>(at + self.vec_ptr),
                mem::read::<usize>(at + self.vec_len),
            )
        }
    }

    /// The `String` at `at`, or empty if it isn't UTF-8.
    ///
    /// # Safety
    ///
    /// As [`Probe::words`], for a `String`.
    unsafe fn string_at(&self, at: usize) -> String {
        // SAFETY: the caller's contract: a `String`'s buffer and length.
        unsafe {
            let (p, n) = self.words(at);
            std::str::from_utf8(std::slice::from_raw_parts(p as *const u8, n))
                .unwrap_or("")
                .to_string()
        }
    }

    /// rekindle's providers registered, of those whose registries are on.
    fn registered(&self) -> String {
        let providers = [
            Provider::Switcher,
            Provider::ChatBar,
            Provider::Menus,
            Provider::Profiles,
            Provider::Modals,
        ];
        let registered = providers
            .iter()
            .filter(|p| provider_index(**p).is_some())
            .count();
        let on = self.on;
        let expected = [on.switcher, on.chat_bar, on.menus, on.profiles, on.modals]
            .iter()
            .filter(|b| **b)
            .count();
        format!("{registered}/{expected}")
    }

    /// The switcher: the client's `rows()` lists rekindle's commands as its
    /// provider's. How many.
    fn switcher_rows(&self) -> Result<Option<usize>> {
        let Some(rows_addr) = self
            .opt("switcher_rows")
            .filter(|_| self.on.switcher && self.have_model)
        else {
            return Ok(None);
        };
        let (hit_size, hit_prov) = (
            self.layout.size("switcher_hit")?,
            self.layout.off("switcher_hit_provider")?,
        );
        let q = b"a";
        let mut vr = [0usize; 3];
        let registry = self.reg("registries_switcher")?;
        // SAFETY: the type's contract: the client's `rows(&self, &ChatModel,
        // &str) -> Vec<Hit>`, with its registry and model, and the `Vec` of
        // `hit_size`-byte hits it returns.
        let n = unsafe {
            let rows_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut u8, *const u8, *const u8, *const u8, usize) -> *mut u8,
            >(rows_addr);
            rows_fn(
                vr.as_mut_ptr().cast::<u8>(),
                registry,
                self.model.as_ptr(),
                q.as_ptr(),
                q.len(),
            );
            let (ptr, len) = self.words(vr.as_ptr() as usize);
            (0..len)
                .filter(|&i| {
                    mem::read::<usize>(ptr + i * hit_size + hit_prov) == ours(Provider::Switcher)
                })
                .count()
        };
        Ok(Some(n))
    }

    /// The chat bar: the client's `buttons()` includes rekindle's, and its
    /// action (rekindle's own closure, in the client's `Rc`) routes the
    /// click. (appeared, routed).
    fn chat_bar_button(&self, driver: &mut Driver) -> Result<(Option<bool>, Option<bool>)> {
        let Some(buttons_addr) = self
            .opt("chatbar_buttons")
            .filter(|_| self.on.chat_bar && self.have_model)
        else {
            return Ok((None, None));
        };
        let (bsize, boff_id, boff_action) = (
            self.layout.size("chatbar_button")?,
            self.layout.off("button_id")?,
            self.layout.off("button_action")?,
        );
        let none_tag = self.layout.tag("client_action_none")?;
        let registry = self.reg("registries_chat_bar")?;
        let mut vb = [0usize; 3];
        // SAFETY: the type's contract: the client's `buttons(&self,
        // &ChatModel) -> Vec<ChatBarButton>`, and the buttons it returns.
        let found = unsafe {
            let buttons_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut u8, *const u8, *const u8) -> *mut u8,
            >(buttons_addr);
            buttons_fn(vb.as_mut_ptr().cast::<u8>(), registry, self.model.as_ptr());
            let (ptr, len) = self.words(vb.as_ptr() as usize);
            (0..len)
                .map(|i| ptr + i * bsize)
                .find(|&b| self.string_at(b + boff_id).starts_with("plugin-"))
        };
        let Some(b) = found else {
            return Ok((Some(false), None));
        };
        let before = rekindle_ui_hook::button_clicks();
        // SAFETY: as above: the button's action is rekindle's own
        // `ButtonAction`, which the client holds; it isn't dropped here.
        let none = unsafe {
            let fat = mem::read::<[usize; 2]>(b + boff_action);
            let action =
                std::mem::ManuallyDrop::new(std::mem::transmute::<[usize; 2], ButtonAction>(fat));
            (action)(self.model.as_ptr())
        };
        driver.pump(); // the plugin opens its modal
        let tag = u64::from_ne_bytes(none[..8].try_into()?);
        Ok((
            Some(true),
            Some(rekindle_ui_hook::button_clicks() > before && tag == none_tag),
        ))
    }

    /// The modal: the client's `top()` is rekindle's dialog, from rekindle's
    /// provider; its `dismiss()` takes it away. (shown, dismissed).
    fn modal(&self, driver: &mut Driver, opened: bool) -> Result<(Option<bool>, Option<bool>)> {
        let Some(top_addr) = self.opt("modals_top").filter(|_| self.on.modals && opened) else {
            return Ok((None, None));
        };
        let (m_prov, m_key) = (
            self.layout.off("modal_off_provider")?,
            self.layout.off("modal_off_key")?,
        );
        let size = self
            .layout
            .size("modal_return")?
            .max(128)
            .max(m_prov + 8)
            .max(m_key + 24);
        let registry = self.reg("registries_modals")?;
        let mut buf = vec![0u8; size];
        let top = |buf: &mut Vec<u8>| -> bool {
            buf.fill(0);
            // SAFETY: the type's contract: the client's `top(&self) ->
            // Option<ModalSpec>`, into a buffer the size of its return value.
            unsafe {
                let top_fn = std::mem::transmute::<
                    usize,
                    unsafe extern "C" fn(*mut u8, *const u8) -> *mut u8,
                >(top_addr);
                top_fn(buf.as_mut_ptr(), registry);
                // None is the key String's capacity niche.
                mem::read::<isize>(buf.as_ptr() as usize + m_key) >= 0
                    && mem::read::<usize>(buf.as_ptr() as usize + m_prov) == ours(Provider::Modals)
            }
        };
        let shown = top(&mut buf);
        let Some(dismiss_addr) = self.opt("modals_dismiss").filter(|_| shown) else {
            return Ok((Some(shown), None));
        };
        // SAFETY: as above: the modal's key, and the client's
        // `dismiss(&self, provider, &str)`.
        unsafe {
            let key = self.string_at(buf.as_ptr() as usize + m_key);
            let dismiss_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*const u8, usize, *const u8, usize),
            >(dismiss_addr);
            dismiss_fn(registry, ours(Provider::Modals), key.as_ptr(), key.len());
        }
        driver.pump();
        Ok((Some(true), Some(!top(&mut buf))))
    }

    /// The profile: the client's `open()` tells rekindle, and its
    /// `sections()` then carries the plugin's section. (opened, section).
    fn profile(&self, driver: &mut Driver) -> Result<(Option<bool>, Option<bool>)> {
        let Some(open_addr) = self
            .opt("profile_open")
            .filter(|_| self.on.profiles && self.have_model)
        else {
            return Ok((None, None));
        };
        let registry = self.reg("registries_profiles")?;
        // SAFETY: the type's contract: the client's `open(&self,
        // &ChatModel, user)`.
        unsafe {
            let open_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*const u8, *const u8, u64),
            >(open_addr);
            open_fn(registry, self.model.as_ptr(), USER);
        }
        driver.pump();
        let Some(sections_addr) = self.opt("profile_sections") else {
            return Ok((Some(true), None));
        };
        let (esize, p_prov) = (
            self.layout.size("profile_element")?,
            self.layout.off("profile_off_provider")?,
        );
        let mut vs = [0usize; 3];
        // SAFETY: as above: the client's `sections(&self, &ChatModel, user)
        // -> Vec<Section>`, and the sections it returns.
        let found = unsafe {
            let sections_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut u8, *const u8, *const u8, u64) -> *mut u8,
            >(sections_addr);
            sections_fn(
                vs.as_mut_ptr().cast::<u8>(),
                registry,
                self.model.as_ptr(),
                USER,
            );
            let (ptr, len) = self.words(vs.as_ptr() as usize);
            (0..len)
                .any(|i| mem::read::<usize>(ptr + i * esize + p_prov) == ours(Provider::Profiles))
        };
        Ok((Some(found), Some(found)))
    }

    /// Settings: the client's `get()` finds rekindle's page, and its
    /// `groups()` (what the sidebar lists) has it under EXTENSIONS.
    /// (page, listed).
    fn settings(&self) -> Result<(Option<bool>, Option<bool>)> {
        if !self.on.settings {
            return Ok((None, None));
        }
        let lbl = self.layout.off("settings_label")?;
        let registry = self.reg("registries_settings")?;
        let page = self.opt("settings_get").map(|get_addr| {
            let id = b"plugins";
            // SAFETY: the type's contract: the client's `get(&self, &str) ->
            // Option<&SettingsSection>`, and the section it returns.
            unsafe {
                let get_fn = std::mem::transmute::<
                    usize,
                    unsafe extern "C" fn(*const u8, *const u8, usize) -> usize,
                >(get_addr);
                let sect = get_fn(registry, id.as_ptr(), id.len());
                sect != 0 && self.string_at(sect + lbl) == "Plugins"
            }
        });
        let Some(groups_addr) = self.opt("settings_groups") else {
            return Ok((page, None));
        };
        let (gsize, gname, gsecs) = (
            self.layout.size("settings_group")?,
            self.layout.off("settings_group_name")?,
            self.layout.off("settings_group_sections")?,
        );
        let mut vg = [0usize; 3];
        // SAFETY: as above: the client's `groups(&self) -> Vec<Group>`, each
        // a `&str` name and a `Vec<&SettingsSection>`.
        let listed = unsafe {
            let groups_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut u8, *const u8) -> *mut u8,
            >(groups_addr);
            groups_fn(vg.as_mut_ptr().cast::<u8>(), registry);
            let (ptr, len) = self.words(vg.as_ptr() as usize);
            (0..len).any(|i| {
                let e = ptr + i * gsize;
                // The group name is a &str: (ptr, len).
                let [np, nl] = mem::read::<[usize; 2]>(e + gname);
                let name = std::str::from_utf8(std::slice::from_raw_parts(np as *const u8, nl))
                    .unwrap_or("");
                let (sp, sl) = self.words(e + gsecs);
                name == "EXTENSIONS"
                    && sl == 1
                    && self.string_at(mem::read::<usize>(sp) + lbl) == "Plugins"
            })
        };
        Ok((page, Some(listed)))
    }

    /// The overlay: the client's `renders()` carries rekindle's layer (or,
    /// where `renders()` is inlined, the registry holds rekindle's entry).
    fn overlay_layer(&self) -> Result<Option<bool>> {
        if !self.on.overlays {
            return Ok(None);
        }
        let registry = self.reg("registries_overlays")?;
        let Some(renders_addr) = self.opt("overlays_renders") else {
            let id = self.layout.off("overlay_id")?;
            // SAFETY: the type's contract: the overlay registry's `Vec` of
            // entries, each with its id `String`.
            let ours = unsafe {
                let (ptr, len) = self.words(registry as usize);
                len == 1 && self.string_at(ptr + id) == "rekindle"
            };
            return Ok(Some(ours));
        };
        let mut vo = [0usize; 3];
        // SAFETY: as above: the client's `renders(&self) -> Vec<Render>`, of
        // two-word renders.
        let ours = unsafe {
            let renders_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut u8, *const u8) -> *mut u8,
            >(renders_addr);
            renders_fn(vo.as_mut_ptr().cast::<u8>(), registry);
            let (ptr, len) = self.words(vo.as_ptr() as usize);
            len == 1 && mem::read::<usize>(ptr) != 0 && mem::read::<usize>(ptr + 8) != 0
        };
        Ok(Some(ours))
    }

    /// The context menu: the client's `build()` for a user target has
    /// rekindle's group, and its `invoke()` of that row routes to the
    /// plugin, which copies the user's URL (queued for the UI thread) and
    /// toasts.
    fn menus(&self, driver: &mut Driver) -> Result<MenuCheck> {
        let mut check = MenuCheck {
            rows: None,
            routed: None,
            copied: None,
            toasted: None,
        };
        if !(self.on.menus && self.have_model) {
            return Ok(check);
        }
        let mut user_target = vec![0u8; self.layout.size("menu_target")?];
        let tid = self.layout.off("menu_target_id")?;
        anyhow::ensure!(
            user_target.len() >= (tid + 8).max(8),
            "menu_target is too small"
        );
        user_target[..8].copy_from_slice(&self.layout.tag("menu_target_user")?.to_ne_bytes());
        user_target[tid..tid + 8].copy_from_slice(&USER.to_ne_bytes());
        let registry = self.reg("registries_menus")?;
        let Some(build_addr) = self.opt("menus_build") else {
            return Ok(check);
        };
        let (ssize, srows, sprov) = (
            self.layout.size("menu_section")?,
            self.layout.off("menu_section_rows")?,
            self.layout.off("menu_section_provider")?,
        );
        let rkey = self.layout.off("menu_row_key")?;
        let mut vm = [0usize; 3];
        let mut first_key = None;
        // SAFETY: the type's contract: the client's `build(&self,
        // &ChatModel, &MenuTarget) -> Vec<MenuSection>`, and its sections'
        // rows.
        let rows = unsafe {
            let build_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(*mut u8, *const u8, *const u8, *const u8) -> *mut u8,
            >(build_addr);
            build_fn(
                vm.as_mut_ptr().cast::<u8>(),
                registry,
                self.model.as_ptr(),
                user_target.as_ptr(),
            );
            let (ptr, len) = self.words(vm.as_ptr() as usize);
            let mut n = 0;
            for sec in (0..len)
                .map(|i| ptr + i * ssize)
                .filter(|&s| mem::read::<usize>(s + sprov) == ours(Provider::Menus))
            {
                let (rptr, rlen) = self.words(sec + srows);
                n += rlen;
                if rlen > 0 && first_key.is_none() {
                    first_key = Some(self.string_at(rptr + rkey));
                }
            }
            n
        };
        check.rows = Some(rows);
        let (Some(key), Some(invoke_addr)) = (first_key, self.opt("menus_invoke")) else {
            return Ok(check);
        };
        let action_size = self.layout.size("client_action").unwrap_or(0);
        let mut ret = vec![0u8; action_size.max(64)];
        // SAFETY: as above: the client's `invoke(&self, &ChatModel,
        // provider, &str, &MenuTarget) -> ClientAction`.
        let tag = unsafe {
            let invoke_fn = std::mem::transmute::<
                usize,
                unsafe extern "C" fn(
                    *mut u8,
                    *const u8,
                    *const u8,
                    usize,
                    *const u8,
                    usize,
                    *const u8,
                ) -> *mut u8,
            >(invoke_addr);
            invoke_fn(
                ret.as_mut_ptr(),
                registry,
                self.model.as_ptr(),
                ours(Provider::Menus),
                key.as_ptr(),
                key.len(),
                user_target.as_ptr(),
            );
            mem::read::<u64>(ret.as_ptr() as usize)
        };
        check.routed = Some(tag == self.layout.tag("client_action_none")?);
        driver.pump();
        let copied = rekindle_ui_hook::pending_clipboard().as_deref()
            == Some(&*format!("https://discord.com/users/{USER}"));
        check.copied = Some(copied);
        check.toasted = Some(
            rekindle_ui_hook::toast_texts()
                .iter()
                .any(|t| t.starts_with("Copied")),
        );
        Ok(check)
    }

    /// Compose: give text-replace a rule through its own panel, then send
    /// through the client's own `send_message` (on another thread: the send
    /// blocks on rekindle's answer, which this worker serves).
    /// (settings widgets, rewritten).
    fn compose(&self, driver: &mut Driver) -> Result<(String, String)> {
        let (Some(send_addr), true) = (self.opt("chatmodel_send"), self.have_model) else {
            return Ok((na(None), na(None)));
        };
        let field = |id: &str| {
            rekindle_ui_hook::panels()
                .iter()
                .filter(|p| p.plugin == TEXT_REPLACE)
                .any(|p| {
                    p.widgets.iter().any(|w| {
                        matches!(w, Widget::Button { id: i, .. } | Widget::TextInput { id: i, .. } if i == id)
                    })
                })
        };
        let had_add = field("add");
        rekindle_ui_hook::report_ui(TEXT_REPLACE, "settings", "add", UiInput::Clicked);
        driver.pump();
        let has_inputs = field("find-0") && field("replace-0");
        rekindle_ui_hook::report_ui(
            TEXT_REPLACE,
            "settings",
            "find-0",
            UiInput::Text("teh".into()),
        );
        rekindle_ui_hook::report_ui(
            TEXT_REPLACE,
            "settings",
            "replace-0",
            UiInput::Text("the".into()),
        );
        driver.pump();
        let model_addr = self.model.as_ptr() as usize;
        let ev_size = self.layout.size("client_event")?;
        let sender = std::thread::spawn(move || {
            let mut ret = vec![0u8; ev_size.max(64)];
            let mut text = String::from("teh plan");
            // SAFETY: the type's contract: the client's `send_message(&mut
            // self, String) -> Option<ClientEvent>`, with the probe's model
            // (which outlives the join below) and a String it takes.
            unsafe {
                let send_fn = std::mem::transmute::<
                    usize,
                    unsafe extern "C" fn(*mut u8, *mut u8, *mut String) -> *mut u8,
                >(send_addr);
                send_fn(ret.as_mut_ptr(), model_addr as *mut u8, &raw mut text);
            }
            std::mem::forget(text); // the callee dropped it
            ret
        });
        while !sender.is_finished() {
            driver.serve_compose(std::time::Duration::from_millis(10));
        }
        let ret = sender.join().map_err(|_| anyhow!("send thread panicked"))?;
        // SAFETY: the event the client returned: its tag, and for a sent
        // message its content `String`.
        let sent = unsafe {
            if mem::read::<u64>(ret.as_ptr() as usize)
                == self.layout.tag("client_event_message_sent")?
            {
                self.string_at(ret.as_ptr() as usize + self.layout.off("client_event_content")?)
            } else {
                String::new()
            }
        };
        Ok((
            (had_add && has_inputs).to_string(),
            (sent == "the plan").to_string(),
        ))
    }
}

/// rekindle's provider index in `p`'s registry, or a value no provider has.
fn ours(p: Provider) -> usize {
    provider_index(p).unwrap_or(usize::MAX - 1)
}

/// rekindle's own panel: first on the settings page, and turning a session
/// plugin off and on through it reaches the runtime.
fn manager_panel(driver: &mut Driver, loaded_ids: &[String]) -> String {
    let panels = rekindle_ui_hook::panels();
    let first = panels
        .first()
        .is_some_and(|p| p.plugin == "rekindle" && p.id == "rekindle");
    let probe = loaded_ids.iter().find(|id| id.as_str() != TEXT_REPLACE);
    match (first, probe) {
        (false, _) => "false".to_string(),
        (true, None) => na(None),
        (true, Some(id)) => {
            let key = format!("dev:{id}");
            let listed = panels[0]
                .widgets
                .iter()
                .any(|w| matches!(w, Widget::Toggle { id: i, value: true, .. } if *i == key));
            rekindle_ui_hook::report_ui("rekindle", "rekindle", &key, UiInput::Toggled(false));
            driver.pump();
            let off = driver.rekindle().info(id).is_none();
            rekindle_ui_hook::report_ui("rekindle", "rekindle", &key, UiInput::Toggled(true));
            driver.pump();
            let back = driver.rekindle().info(id).is_some();
            (listed && off && back).to_string()
        }
    }
}
