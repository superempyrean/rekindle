//! What the extension and rekindle-native are configured with: the client's
//! functions from the anchors, and its values' layout from the layout file.

use crate::hooks::{Hooks, frame_hook};
use crate::install::{installed_mode, store_mode};
use crate::layout::Layout;

/// rekindle's real elements: the client's own compiled gpui functions, and
/// gpui's layout. Validated here (headless runs draw no frames, so this is
/// what the self-test can check); used from the `draw_roots` hook live. Every
/// rekindle surface renders through it, so nothing is installed without it.
pub(crate) fn configure_native(layout: &Layout, hooks: &Hooks) -> anyhow::Result<()> {
    let opt = |name: &str| hooks.addr(name).unwrap_or(0);
    let native = rekindle_native::Fns {
        interactivity_new: hooks.addr("gpui_interactivity_new")?,
        div_into_any_element: hooks.addr("gpui_div_into_any_element")?,
        str_into_any_element: hooks.addr("gpui_str_into_any_element")?,
        dispatch_event: hooks.addr("gpui_window_dispatch_event")?,
        window_focus: hooks.addr("gpui_window_focus")?,
        window_blur: hooks.addr("gpui_window_blur")?,
        window_refresh: hooks.addr("gpui_window_refresh")?,
        // Whichever of the two the build kept out of line.
        focus_handle_new: opt("gpui_focus_handle_new"),
        app_focus_handle: opt("gpui_app_focus_handle"),
        focus_handle_clone: hooks.addr("gpui_focus_handle_clone")?,
        wake_platform: hooks.addr("gpui_invalidator_wake_platform")?,
        window_dispatch_action: opt("gpui_window_dispatch_action"),
        paint_quad: opt("gpui_window_paint_quad"),
        paint_image: opt("gpui_window_paint_image"),
    };
    rekindle_native::spec_from_maps(
        native,
        frame_hook(hooks)?.1,
        &layout.offsets,
        &layout.sizes,
        &layout.tags,
        &layout.templates,
    )
    .and_then(rekindle_native::configure)
    .map_err(|e| anyhow::anyhow!("rekindle-native: {e:#}"))
}

/// What rekindle runs inside the client: the user's installed plugins (a
/// real launch), or the plugins the test named, in memory and offline.
pub(crate) fn ext_args() -> (rekindle_extension::ExtArgs, usize) {
    if installed_mode() || store_mode() {
        let ext = rekindle_extension::ExtArgs {
            dev_paths: Vec::new(),
            load_installed: true,
            memory_storage: false,
            allow_network: true,
            wait_for_client: false,
            home: None,
        };
        return (ext, 0);
    }
    let dev_paths: Vec<std::path::PathBuf> = std::env::var("REKINDLE_HOST_PLUGINS")
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .collect();
    let n = dev_paths.len();
    (
        rekindle_extension::ExtArgs {
            dev_paths,
            load_installed: false,
            memory_storage: true,
            allow_network: false,
            wait_for_client: false,
            home: None,
        },
        n,
    )
}

/// Where rekindle's providers and entries go in the client's registries, and
/// how the values they hand the client are laid out. All from the layout.
/// A registry whose entries aren't all known is off in `surfaces`, and the
/// second value says which and why. Without the `Vec` layout, which every
/// registry needs, it's an error.
#[expect(
    clippy::too_many_lines,
    reason = "one block per registry, each a list of layout entries"
)]
pub(crate) fn registry_layout(
    layout: &Layout,
    with_builtins_addr: usize,
) -> anyhow::Result<(rekindle_ui_hook::registry::RegistryLayout, Vec<String>)> {
    use rekindle_ui_hook::VecLayout;
    use rekindle_ui_hook::registry::{ACTION, RegistryLayout};
    let vec = VecLayout {
        cap_off: layout.off("gpui_vec_cap")?,
        ptr_off: layout.off("gpui_vec_ptr")?,
        len_off: layout.off("gpui_vec_len")?,
    };
    let mut l = RegistryLayout {
        with_builtins_addr,
        vec,
        page_id: "plugins".into(),
        page_label: "Plugins".into(),
        page_group: "EXTENSIONS".into(),
        ..Default::default()
    };
    let mut off = Vec::new();
    let mut part =
        |name: &str,
         l: &mut RegistryLayout,
         fill: &dyn Fn(&mut RegistryLayout) -> anyhow::Result<()>| match fill(l) {
            Ok(()) => true,
            Err(e) => {
                off.push(format!(
                    "{name}({})",
                    format!("{e:#}").replace([' ', '"'], "_")
                ));
                false
            }
        };
    // The client's action, which the switcher, chat bar and menus return.
    let action = |l: &mut RegistryLayout| -> anyhow::Result<()> {
        l.action_size = layout.size("client_action")?;
        anyhow::ensure!(
            l.action_size == ACTION,
            "the client's action is {} bytes, not {ACTION}",
            l.action_size
        );
        l.action_none_tag = layout.tag("client_action_none")?;
        Ok(())
    };
    l.surfaces.switcher = part("switcher", &mut l, &|l| {
        action(l)?;
        l.switcher = layout.off("registries_switcher")?;
        l.row_size = layout.size("switcher_row")?;
        l.row_key = layout.off("switcher_row_key")?;
        l.row_glyph = layout.off("switcher_row_glyph")?;
        l.row_title = layout.off("switcher_row_title")?;
        l.row_detail = layout.off("switcher_row_detail")?;
        Ok(())
    });
    l.surfaces.chat_bar = part("chat_bar", &mut l, &|l| {
        action(l)?;
        l.chat_bar = layout.off("registries_chat_bar")?;
        l.chat_bar_providers = layout.off("chat_bar_providers")?;
        l.button_size = layout.size("chatbar_button")?;
        l.button_id = layout.off("button_id")?;
        l.button_label = layout.off("button_label")?;
        l.button_tooltip = layout.off("button_tooltip")?;
        l.button_order = layout.off("button_order")?;
        l.button_action = layout.off("button_action")?;
        Ok(())
    });
    l.surfaces.menus = part("menus", &mut l, &|l| {
        action(l)?;
        l.menus = layout.off("registries_menus")?;
        l.menu_row_size = layout.size("menu_row")?;
        l.menu_row_key = layout.off("menu_row_key")?;
        l.menu_row_label = layout.off("menu_row_label")?;
        l.menu_row_danger = layout.off("menu_row_danger")?;
        l.target_tag_message = layout.tag("menu_target_message")?;
        l.target_tag_user = layout.tag("menu_target_user")?;
        l.target_tag_channel = layout.tag("menu_target_channel")?;
        l.target_id = layout.off("menu_target_id")?;
        l.target_message_channel = layout.off("menu_target_message_channel")?;
        l.target_message_author = layout.off("menu_target_message_author")?;
        Ok(())
    });
    l.surfaces.profiles = part("profiles", &mut l, &|l| {
        l.profiles = layout.off("registries_profiles")?;
        l.profile_size = layout.size("profile_section")?;
        l.profile_id = layout.off("profile_section_id")?;
        l.profile_title = layout.off("profile_section_title")?;
        l.profile_body = layout.off("profile_section_body")?;
        l.profile_body_template = layout.template("profile_body_render")?;
        l.profile_body_payload = layout.off("profile_body_payload")?;
        Ok(())
    });
    l.surfaces.modals = part("modals", &mut l, &|l| {
        l.modals = layout.off("registries_modals")?;
        l.modal_size = layout.size("modal_spec")?;
        l.modal_key = layout.off("modal_spec_key")?;
        l.modal_title = layout.off("modal_spec_title")?;
        l.modal_caption = layout.off("modal_spec_caption")?;
        l.modal_body = layout.off("modal_spec_body")?;
        Ok(())
    });
    l.surfaces.settings = part("settings", &mut l, &|l| {
        l.settings = layout.off("registries_settings")?;
        l.section_size = layout.size("settings_section")?;
        l.section_id = layout.off("settings_id")?;
        l.section_label = layout.off("settings_label")?;
        l.section_group = layout.off("settings_group")?;
        l.section_order = layout.off("settings_order")?;
        l.section_render = layout.off("settings_render")?;
        Ok(())
    });
    l.surfaces.overlays = part("overlays", &mut l, &|l| {
        l.overlays = layout.off("registries_overlays")?;
        l.overlay_size = layout.size("overlay")?;
        l.overlay_id = layout.off("overlay_id")?;
        l.overlay_render = layout.off("overlay_render")?;
        Ok(())
    });
    l.surfaces.decorators = part("decorators", &mut l, &|l| {
        l.decorators = layout.off("registries_decorators")?;
        l.decorators_providers = layout.off("decorators_providers")?;
        l.look_size = layout.size("message_look")?;
        l.look_badges = layout.off("look_badges")?;
        l.look_highlights = layout.off("look_highlights")?;
        l.look_name_color = layout.off("look_name_color")?;
        l.look_tint = layout.off("look_tint")?;
        l.look_append = layout.off("look_append")?;
        l.look_replace = layout.off("look_replace")?;
        l.look_collapse = layout.off("look_collapse")?;
        l.look_blur = layout.off("look_blur")?;
        l.look_by = layout.off("look_by")?;
        l.badge_size = layout.size("badge")?;
        l.badge_text = layout.off("badge_text")?;
        l.badge_tooltip = layout.off("badge_tooltip")?;
        l.badge_color = layout.off("badge_color")?;
        l.highlight_size = layout.size("highlight")?;
        l.highlight_start = layout.off("highlight_start")?;
        l.highlight_end = layout.off("highlight_end")?;
        l.highlight_color = layout.off("highlight_color")?;
        Ok(())
    });
    Ok((l, off))
}

/// Where the client keeps what plugins' queries read, from the layout
/// file. `Err` when any entry is missing: then no query reads the client.
pub(crate) fn client_layout(
    layout: &Layout,
) -> anyhow::Result<rekindle_ui_hook::client::ClientLayout> {
    use rekindle_ui_hook::VecLayout;
    use rekindle_ui_hook::client::ClientLayout;
    Ok(ClientLayout {
        vec: VecLayout {
            cap_off: layout.off("gpui_vec_cap")?,
            ptr_off: layout.off("gpui_vec_ptr")?,
            len_off: layout.off("gpui_vec_len")?,
        },
        model_data: layout.off("chatmodel_data")?,
        model_guild: layout.off("chatmodel_guild")?,
        model_channel: layout.off("chatmodel_channel")?,
        data_users: layout.off("fakedata_users")?,
        data_guilds: layout.off("fakedata_guilds")?,
        user_size: layout.size("user")?,
        user_id: layout.off("user_id")?,
        user_username: layout.off("user_username")?,
        user_display: layout.off("user_display")?,
        user_bot: layout.off("user_bot")?,
        guild_size: layout.size("guild")?,
        guild_id: layout.off("guild_id")?,
        guild_name: layout.off("guild_name")?,
        guild_owner: layout.off("guild_owner")?,
        guild_roles: layout.off("guild_roles")?,
        guild_members: layout.off("guild_members")?,
        role_size: layout.size("role")?,
        role_id: layout.off("role_id")?,
        role_name: layout.off("role_name")?,
        role_color: layout.off("role_color")?,
        role_hoist: layout.off("role_hoist")?,
        role_permissions: layout.off("role_permissions")?,
        member_size: layout.size("member")?,
        member_user: layout.off("member_user")?,
        member_roles: layout.off("member_roles")?,
        member_nick: layout.off("member_nick")?,
        message_id: layout.off("message_id")?,
        message_author: layout.off("message_author")?,
        message_body: layout.off("message_body")?,
        message_edited: layout.off("message_edited")?,
        message_pinned: layout.off("message_pinned")?,
    })
}
