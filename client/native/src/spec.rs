//! The recovered layout entries, by name as in `layout.json`, and their
//! check. [`Spec::resolve`] turns them into a typed [`Resolved`] layout once,
//! up front, and refuses a layout file that is missing an entry or whose
//! offsets would have rekindle read or write past the value they are in.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};

use crate::layout::{
    ActionLayout, ClientLayout, EventLayout, ImageLayout, InputLayout, Interaction, KeyLayout,
    PaintLayout, Resolved, ScrollLayout, StyleOffsets, Tags,
};

/// An `AnyElement`'s size: three words.
const ANY_ELEMENT: usize = 24;

/// Addresses of the client's own compiled gpui functions (resolved anchors).
//
// These are what survive an optimized build: `div()`, the `into_any`s, the
// `.child()` path and `Hitbox::is_hovered` are inlined away under LTO, so
// rekindle calls what they call, or does their few stores itself.
#[derive(Clone, Copy, Debug)]
pub struct Fns {
    /// `Interactivity::new(ret, caller: &'static Location)`: a `Div` is its
    /// `Interactivity` (at +0) followed by fields whose empty value is all
    /// zeros (children `SmallVec` with capacity 0, `None` boxes), so `div()`
    /// is this on a zeroed buffer. `#[track_caller]`: a hidden `Location`
    /// follows sret (stored only in debug builds; harmless otherwise).
    pub interactivity_new: usize,
    /// `<Div as IntoElement>::into_any_element(ret: *mut AnyElement, div:
    /// *const Div)`. Consumes the `Div` (copies it into the element arena).
    pub div_into_any_element: usize,
    /// `<&str as IntoElement>::into_any_element(ret, ptr, len)`.
    pub str_into_any_element: usize,
    /// `Window::dispatch_event(&mut self, PlatformInput, &mut App)`.
    pub dispatch_event: usize,
    /// `Window::focus(&mut self, &FocusHandle, &mut App)`.
    pub window_focus: usize,
    /// `Window::blur(&mut self, &mut App)`.
    pub window_blur: usize,
    /// `Window::refresh(&mut self)`.
    pub window_refresh: usize,
    /// `FocusHandle::new(ret, &Arc<FocusMap>)` (what `App::focus_handle`
    /// calls); 0 if inlined away, and then `app_focus_handle` is used.
    pub focus_handle_new: usize,
    /// `App::focus_handle(ret, &App)`; 0 where it is a no-`.pdata` thunk.
    pub app_focus_handle: usize,
    /// `<FocusHandle as Clone>::clone(ret, &FocusHandle)`.
    pub focus_handle_clone: usize,
    /// `Window::dispatch_action(&mut self, Box<dyn Action>, &mut App)`; 0 if
    /// absent (then actions can't be dispatched by name).
    pub window_dispatch_action: usize,
    /// `WindowInvalidator::wake_platform(&self)`.
    pub wake_platform: usize,
    /// `Window::paint_quad(&mut self, PaintQuad)`; 0 if absent (then
    /// rekindle doesn't paint quads itself, `crate::can_paint`).
    pub paint_quad: usize,
    /// `Window::paint_image`; 0 if absent (a client that draws no images
    /// doesn't have it, and then rekindle draws none, `crate::can_draw_images`).
    pub paint_image: usize,
}

/// Everything recovered about the client's gpui: its functions, and the
/// layout entries by name, as in `layout.json` ([`spec_from_maps`]).
#[derive(Clone, Debug)]
pub struct Spec {
    fns: Fns,
    /// An address in the client's code (e.g. `Window::draw_roots`): a window
    /// root's `render` pointer must lie close to it, or it isn't a render fn.
    near: usize,
    offsets: BTreeMap<String, usize>,
    sizes: BTreeMap<String, usize>,
    tags: BTreeMap<String, usize>,
    templates: BTreeMap<String, Vec<u8>>,
}

/// A [`Spec`] from the layout file's maps, as the host loaded them.
/// `templates` are hex strings, as in `layout.json`.
pub fn spec_from_maps(
    fns: Fns,
    near: usize,
    offsets: &BTreeMap<String, usize>,
    sizes: &BTreeMap<String, usize>,
    tags: &BTreeMap<String, usize>,
    templates: &BTreeMap<String, String>,
) -> Result<Spec> {
    let templates = templates
        .iter()
        .map(|(k, hex)| {
            let bytes = from_hex(hex).with_context(|| format!("template {k:?} is not hex"))?;
            Ok((k.clone(), bytes))
        })
        .collect::<Result<_>>()?;
    Ok(Spec {
        fns,
        near,
        offsets: offsets.clone(),
        sizes: sizes.clone(),
        tags: tags.clone(),
        templates,
    })
}

/// Pairs of hex digits as bytes; `None` for anything else.
fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let digit = |c: u8| char::from(c).to_digit(16);
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[hi, lo]| Some((digit(hi)? << 4 | digit(lo)?) as u8))
        .collect()
}

/// `Err` naming `what` unless every `(offset, width)` field lies inside
/// `size` bytes.
fn fits(what: &str, size: usize, fields: &[(usize, usize)]) -> Result<()> {
    match fields.iter().find(|(off, width)| off + width > size) {
        Some((off, width)) => {
            bail!("{what}: a field at {off} ({width} bytes) runs past its {size} bytes")
        }
        None => Ok(()),
    }
}

/// Whether three offsets are a `Vec`'s or `String`'s words, in some order.
fn three_words(words: [usize; 3]) -> bool {
    let mut sorted = words;
    sorted.sort_unstable();
    sorted == [0, 8, 16]
}

impl Spec {
    fn off(&self, name: &str) -> Result<usize> {
        self.offsets
            .get(name)
            .copied()
            .with_context(|| format!("layout has no offset {name:?}"))
    }

    fn size(&self, name: &str) -> Result<usize> {
        self.sizes
            .get(name)
            .copied()
            .with_context(|| format!("layout has no size {name:?}"))
    }

    fn tag(&self, name: &str) -> Result<usize> {
        self.tags
            .get(name)
            .copied()
            .with_context(|| format!("layout has no tag {name:?}"))
    }

    fn tag8(&self, name: &str) -> Result<u8> {
        u8::try_from(self.tag(name)?).with_context(|| format!("tag {name:?} does not fit a byte"))
    }

    fn tag32(&self, name: &str) -> Result<u32> {
        u32::try_from(self.tag(name)?).with_context(|| format!("tag {name:?} does not fit 32 bits"))
    }

    fn template(&self, name: &str) -> Result<Vec<u8>> {
        self.templates
            .get(name)
            .cloned()
            .with_context(|| format!("layout has no template {name:?}"))
    }

    /// Whether the layout file has `section:name`.
    fn has(&self, entry: &str) -> bool {
        match entry.split_once(':') {
            Some(("offsets", name)) => self.offsets.contains_key(name),
            Some(("sizes", name)) => self.sizes.contains_key(name),
            Some(("tags", name)) => self.tags.contains_key(name),
            Some(("templates", name)) => self.templates.contains_key(name),
            _ => false,
        }
    }

    /// An optional group of layout entries: `None` where the layout file has
    /// none of `entries`, an error where it has some but not all.
    fn optional<T>(
        &self,
        entries: &[&str],
        build: impl FnOnce() -> Result<T>,
    ) -> Result<Option<T>> {
        let present = entries.iter().filter(|e| self.has(e)).count();
        match present {
            0 => Ok(None),
            n if n == entries.len() => build().map(Some),
            _ => bail!(
                "the layout file has {present} of the {} entries of {}",
                entries.len(),
                entries[0]
            ),
        }
    }

    /// Check the layout and type it. Refuses a layout file that lacks a
    /// required entry, has part of an optional group, or would have rekindle
    /// write past the value it writes.
    pub(crate) fn resolve(self) -> Result<Resolved> {
        let f = &self.fns;
        if [
            f.interactivity_new,
            f.div_into_any_element,
            f.str_into_any_element,
            f.dispatch_event,
            f.window_focus,
            f.window_blur,
            f.window_refresh,
            f.focus_handle_new.max(f.app_focus_handle),
            f.focus_handle_clone,
            f.wake_platform,
            self.near,
        ]
        .contains(&0)
        {
            bail!("a gpui function address is missing");
        }
        let style_size = self.size("gpui_style_refinement")?;
        let div_size = self.size("gpui_div")?;
        let base_style = self.off("gpui_div_base_style")?;
        let children = self.off("gpui_div_children")?;
        if !(64..=16384).contains(&div_size)
            || base_style + 8 > div_size
            || !base_style.is_multiple_of(8)
            || children + 24 > div_size
            || !children.is_multiple_of(8)
        {
            bail!(
                "implausible Div layout: size {div_size}, base_style at {base_style}, children at {children}"
            );
        }
        let s = self.style_offsets()?;
        let t = self.tags()?;
        let i = self.interaction(div_size)?;
        let c = self.client_layout()?;
        if c.children_inline == 0 {
            bail!("the Div's children have no inline capacity");
        }
        fits(
            "the Div's children",
            div_size - children,
            &[
                (0, c.children_inline * ANY_ELEMENT),
                (c.children_capacity, 8),
            ],
        )?;
        let fill = self.template("gpui_some_fill")?;
        Self::check_style(&s, style_size, &fill, c.fill_hsla)?;
        Ok(Resolved {
            fns: self.fns,
            near: self.near,
            window_root: c.window_root,
            view_render: self.off("gpui_anyview_render")?,
            div_size,
            base_style,
            children,
            i,
            style_size,
            s,
            t,
            fill,
            scroll: self.scroll(style_size, div_size)?,
            paint: self.paint()?,
            image: self.image()?,
            c,
        })
    }

    fn style_offsets(&self) -> Result<StyleOffsets> {
        let st = |name: &str| self.off(&format!("gpui_style_{name}"));
        let text = st("text")?;
        let edges = |base: usize| -> Result<[usize; 4]> {
            Ok([
                base + self.off("gpui_edges_top")?,
                base + self.off("gpui_edges_right")?,
                base + self.off("gpui_edges_bottom")?,
                base + self.off("gpui_edges_left")?,
            ])
        };
        let corners = st("corner_radii")?;
        Ok(StyleOffsets {
            text_color: text + self.off("gpui_text_color")?,
            text_size: text + self.off("gpui_text_font_size")?,
            text_line_height: text + self.off("gpui_text_line_height")?,
            background: st("background")?,
            border_widths: edges(st("border_widths")?)?,
            border_color: st("border_color")?,
            padding: edges(st("padding")?)?,
            inset: edges(st("inset")?)?,
            corners: [
                corners + self.off("gpui_corners_top_left")?,
                corners + self.off("gpui_corners_top_right")?,
                corners + self.off("gpui_corners_bottom_right")?,
                corners + self.off("gpui_corners_bottom_left")?,
            ],
            width: st("size")? + self.off("gpui_size_width")?,
            height: st("size")? + self.off("gpui_size_height")?,
            gap: st("gap")?,
            position: st("position")?,
            display: st("display")?,
            visibility: st("visibility")?,
            flex_direction: st("flex_direction")?,
            align_items: st("align_items")?,
            justify_content: st("justify_content")?,
            font_weight: text + self.off("gpui_text_font_weight")?,
            mouse_cursor: st("mouse_cursor")?,
        })
    }

    /// Every write `write_style` makes lands inside the style box: options
    /// of a colour are a u32 tag and four f32s, of a length a u32 tag and an
    /// f32, and the background is the whole `Some(Fill)` template.
    fn check_style(
        s: &StyleOffsets,
        style_size: usize,
        fill: &[u8],
        fill_hsla: usize,
    ) -> Result<()> {
        let mut fields = vec![
            (s.text_color, 20),
            (s.border_color, 20),
            (s.font_weight, 8),
            (s.text_size, 8),
            (s.text_line_height, 8),
            (s.background, fill.len()),
            (s.gap, 16),
            (s.width, 8),
            (s.height, 8),
        ];
        for off in s
            .padding
            .iter()
            .chain(&s.inset)
            .chain(&s.corners)
            .chain(&s.border_widths)
        {
            fields.push((*off, 8));
        }
        for off in [
            s.mouse_cursor,
            s.position,
            s.display,
            s.visibility,
            s.flex_direction,
            s.align_items,
            s.justify_content,
        ] {
            fields.push((off, 1));
        }
        fits("the style", style_size, &fields)?;
        if fill.len() != 72 || fill_hsla + 16 > fill.len() {
            bail!(
                "the fill template is malformed: {} bytes, colour at {fill_hsla}",
                fill.len()
            );
        }
        Ok(())
    }

    fn tags(&self) -> Result<Tags> {
        Ok(Tags {
            len_pixels: self.tag32("gpui_len_pixels")?,
            len_fraction: self.tag32("gpui_len_fraction")?,
            len_auto: self.tag32("gpui_len_auto")?,
            option_some: self.tag32("gpui_option_hsla_some")?,
            position_absolute: self.tag8("gpui_position_absolute")?,
            display_flex: self.tag8("gpui_display_flex")?,
            flex_row: self.tag8("gpui_flex_row")?,
            flex_column: self.tag8("gpui_flex_column")?,
            align_start: self.tag8("gpui_align_start")?,
            align_end: self.tag8("gpui_align_end")?,
            align_center: self.tag8("gpui_align_center")?,
            phase_bubble: self.tag8("gpui_phase_bubble")?,
            hitbox_block_mouse: self.tag8("gpui_hitbox_block_mouse")?,
            font_weight_some: self.tag32("gpui_font_weight_some")?,
            cursor_text: self.tag8("gpui_cursor_text")?,
            cursor_pointer: self.tag8("gpui_cursor_pointer")?,
            visibility_visible: self.tag8("gpui_visibility_visible")?,
            visibility_hidden: self.tag8("gpui_visibility_hidden")?,
            shared_string_static: self.tag8("gpui_shared_string_static")?,
        })
    }

    fn interaction(&self, div_size: usize) -> Result<Interaction> {
        let i = Interaction {
            mouse_down_listeners: self.off("gpui_div_mouse_down_listeners")?,
            hover_style: self.off("gpui_div_hover_style")?,
            hitbox_behavior: self.off("gpui_div_hitbox_behavior")?,
            group: self.off("gpui_div_group")?,
            group_hover_style: self.off("gpui_div_group_hover_style")?,
            group_style_box: self.off("gpui_group_style_style")?,
            shared_string_ptr: self.off("gpui_shared_string_ptr")?,
            shared_string_len: self.off("gpui_shared_string_len")?,
            key_down_listeners: self.off("gpui_div_key_down_listeners")?,
            tracked_focus_handle: self.off("gpui_div_tracked_focus_handle")?,
            focusable: self.off("gpui_div_focusable")?,
        };
        fits(
            "a static SharedString",
            24,
            &[(i.shared_string_ptr, 8), (i.shared_string_len, 8)],
        )?;
        fits(
            "the Div",
            div_size,
            &[
                (i.mouse_down_listeners, 24),
                (i.key_down_listeners, 24),
                (i.hover_style, 8),
                (i.hitbox_behavior, 1),
                (i.group, 24),
                (i.group_hover_style, 24),
                (i.group_hover_style + i.group_style_box, 8),
                (i.tracked_focus_handle, 32),
                (i.focusable, 1),
            ],
        )?;
        Ok(i)
    }

    fn client_layout(&self) -> Result<ClientLayout> {
        let c = ClientLayout {
            viewport_size: self.off("gpui_window_viewport_size")?,
            scale_factor: self.off("gpui_window_scale_factor")?,
            mouse_position: self.off("gpui_window_mouse_position")?,
            focus: self.off("gpui_window_focus")?,
            hitboxes: self.off("gpui_window_next_frame_hitboxes")?,
            rendered_hitboxes: self.off("gpui_window_rendered_frame_hitboxes")?,
            rendered_scene: self.off("gpui_window_rendered_frame_scene")?,
            scene: self.off("gpui_window_next_frame_scene")?,
            next_frame_callbacks: self.off("gpui_window_next_frame_callbacks")?,
            refcell_borrow: self.off("gpui_rc_refcell_borrow")?,
            refcell_vec: self.off("gpui_rc_refcell_vec")?,
            platform_window: self.off("gpui_window_platform_window")?,
            schedule_frame_slot: self.off("gpui_platform_window_schedule_frame")?,
            invalidator: self.off("gpui_window_invalidator")?,
            propagate_event: self.off("gpui_app_propagate_event")?,
            focus_handles: self.off("gpui_app_focus_handles")?,
            focus_handle_size: self.size("gpui_focus_handle")?,
            quads: self.off("gpui_scene_quads")?,
            shadows: self.off("gpui_scene_shadows")?,
            underlines: self.off("gpui_scene_underlines")?,
            mono: self.off("gpui_scene_monochrome_sprites")?,
            subpx: self.off("gpui_scene_subpixel_sprites")?,
            quad: (
                self.size("gpui_quad")?,
                self.off("gpui_quad_bounds")?,
                self.off("gpui_quad_background")?,
                self.off("gpui_quad_border_color")?,
            ),
            shadow: (self.size("gpui_shadow")?, self.off("gpui_shadow_color")?),
            underline: (
                self.size("gpui_underline")?,
                self.off("gpui_underline_color")?,
            ),
            mono_sprite: (
                self.size("gpui_mono_sprite")?,
                self.off("gpui_mono_sprite_color")?,
            ),
            subpx_sprite: (
                self.size("gpui_subpx_sprite")?,
                self.off("gpui_subpx_sprite_color")?,
            ),
            sprite_bounds: (
                self.off("gpui_mono_sprite_bounds")?,
                self.off("gpui_subpx_sprite_bounds")?,
            ),
            sprite_order: (
                self.off("gpui_mono_sprite_order")?,
                self.off("gpui_subpx_sprite_order")?,
            ),
            poly: self.poly()?,
            poly_tile: self.poly_tile()?,
            quad_order: self.off("gpui_quad_order")?,
            background_solid_tag: self.tag32("gpui_background_solid")?,
            fill_hsla: self.off("gpui_fill_hsla")?,
            hitbox: (self.size("gpui_hitbox")?, self.off("gpui_hitbox_bounds")?),
            mouse_button: self.off("gpui_mouse_down_button")?,
            button_left: self.tag8("gpui_button_left")?,
            events: self.events()?,
            input: self.input()?,
            key: self.key()?,
            vec: [
                self.off("gpui_vec_cap")?,
                self.off("gpui_vec_ptr")?,
                self.off("gpui_vec_len")?,
            ],
            hitbox_id: self.off("gpui_hitbox_id")?,
            captured_hitbox: self.off("gpui_window_captured_hitbox")?,
            option_some_tag: self.tag32("gpui_option_hitbox_some")?,
            last_input_modality: self.off("gpui_window_last_input_modality")?,
            modality_keyboard: self.tag8("gpui_input_modality_keyboard")?,
            mouse_hit_test: self.off("gpui_window_mouse_hit_test")?,
            hit_test_ids: self.off("gpui_hit_test_ids")?,
            ids_capacity: self.off("gpui_hit_test_ids_capacity")?,
            ids_inline: self.size("gpui_hit_test_ids_inline")?,
            hit_test_hover_count: self.off("gpui_hit_test_hover_count")?,
            children_capacity: self.off("gpui_children_capacity")?,
            children_inline: self.size("gpui_children_inline")?,
            actions: self.actions()?,
            view_type_name: self.off("gpui_anyview_view_type_name")?,
            window_root: self.off("gpui_window_root")?,
        };
        if !three_words(c.vec) {
            bail!("Vec header is not three words: {:?}", c.vec);
        }
        if !three_words(c.key.string) {
            bail!("String header is not three words: {:?}", c.key.string);
        }
        Self::check_frame(&c)?;
        if let Some(e) = &c.events {
            Self::check_events(e, &c)?;
        }
        Ok(c)
    }

    /// The scene's images: where the list is, a sprite's size and its bounds.
    fn poly(&self) -> Result<Option<(usize, usize, usize)>> {
        self.optional(
            &[
                "offsets:gpui_scene_polychrome_sprites",
                "sizes:gpui_poly_sprite",
                "offsets:gpui_poly_sprite_bounds",
            ],
            || {
                Ok((
                    self.off("gpui_scene_polychrome_sprites")?,
                    self.size("gpui_poly_sprite")?,
                    self.off("gpui_poly_sprite_bounds")?,
                ))
            },
        )
    }

    /// An image's atlas tile: where it is in the sprite, and its bounds and
    /// padding.
    fn poly_tile(&self) -> Result<Option<(usize, usize, usize)>> {
        self.optional(
            &[
                "offsets:gpui_poly_sprite_tile",
                "offsets:gpui_atlas_tile_bounds",
                "offsets:gpui_atlas_tile_padding",
            ],
            || {
                Ok((
                    self.off("gpui_poly_sprite_tile")?,
                    self.off("gpui_atlas_tile_bounds")?,
                    self.off("gpui_atlas_tile_padding")?,
                ))
            },
        )
    }

    fn key(&self) -> Result<KeyLayout> {
        Ok(KeyLayout {
            key: self.off("gpui_keystroke_key")?,
            key_char: self.off("gpui_keystroke_key_char")?,
            modifiers: self.off("gpui_keystroke_modifiers")?,
            control: self.off("gpui_modifiers_control")?,
            alt: self.off("gpui_modifiers_alt")?,
            shift: self.off("gpui_modifiers_shift")?,
            platform: self.off("gpui_modifiers_platform")?,
            string: [
                self.off("gpui_string_cap")?,
                self.off("gpui_string_ptr")?,
                self.off("gpui_string_len")?,
            ],
        })
    }

    /// Every field the frame record is read at lies inside its element.
    fn check_frame(c: &ClientLayout) -> Result<()> {
        let (size, bounds, background, border) = c.quad;
        fits(
            "a quad",
            size,
            &[
                (bounds, 16),
                (background, 4),
                (background + c.fill_hsla, 16),
                (border, 16),
                (c.quad_order, 4),
            ],
        )?;
        for (what, (size, color), bounds, order) in [
            (
                "a monochrome sprite",
                c.mono_sprite,
                c.sprite_bounds.0,
                c.sprite_order.0,
            ),
            (
                "a subpixel sprite",
                c.subpx_sprite,
                c.sprite_bounds.1,
                c.sprite_order.1,
            ),
        ] {
            fits(what, size, &[(color, 16), (bounds, 16), (order, 4)])?;
        }
        fits("a shadow", c.shadow.0, &[(c.shadow.1, 16)])?;
        fits("an underline", c.underline.0, &[(c.underline.1, 16)])?;
        fits(
            "a hitbox",
            c.hitbox.0,
            &[(c.hitbox.1, 16), (c.hitbox_id, 8)],
        )?;
        if let Some((_, size, bounds)) = c.poly {
            let mut fields = vec![(bounds, 16)];
            if let Some((tile, tile_bounds, padding)) = c.poly_tile {
                fields.extend([(tile + tile_bounds, 16), (tile + padding, 4)]);
            }
            fits("a polychrome sprite", size, &fields)?;
        }
        Ok(())
    }

    /// Every field a built or read event has lies inside a `PlatformInput`.
    fn check_events(e: &EventLayout, c: &ClientLayout) -> Result<()> {
        if e.size < 32 {
            bail!("PlatformInput layout is implausible: {} bytes", e.size);
        }
        let (moved, down, up) = e.payload;
        let (key, k) = (e.key_down.1, &c.key);
        let modifiers = key + k.modifiers;
        fits(
            "a PlatformInput",
            e.size,
            &[
                (moved, 8),
                (moved + e.move_pressed_button, 1),
                (down, 8),
                (down + e.click_count, 8),
                (down + c.mouse_button, 1),
                (up, 8),
                (up + e.click_count, 8),
                (up + c.mouse_button, 1),
                (key + k.key, 24),
                (key + k.key_char, 24),
                (
                    modifiers + k.control.max(k.alt).max(k.shift).max(k.platform),
                    1,
                ),
            ],
        )
    }

    /// The focused field's input handler, for `insert-text`.
    fn input(&self) -> Result<Option<InputLayout>> {
        const ENTRIES: [&str; 6] = [
            "offsets:gpui_platform_window_take_input_handler",
            "offsets:gpui_platform_window_set_input_handler",
            "sizes:gpui_platform_input_handler",
            "offsets:gpui_platform_input_handler_handler",
            "offsets:gpui_platform_input_handler_focus_id",
            "offsets:gpui_input_handler_replace_text_in_range",
        ];
        self.optional(&ENTRIES, || {
            let l = InputLayout {
                take_slot: self.off("gpui_platform_window_take_input_handler")?,
                set_slot: self.off("gpui_platform_window_set_input_handler")?,
                size: self.size("gpui_platform_input_handler")?,
                handler: self.off("gpui_platform_input_handler_handler")?,
                focus_id: self.off("gpui_platform_input_handler_focus_id")?,
                replace_slot: self.off("gpui_input_handler_replace_text_in_range")?,
            };
            fits(
                "the PlatformInputHandler",
                l.size,
                &[(0, 8), (l.handler, 16), (l.focus_id, 8)],
            )?;
            Ok(l)
        })
    }

    /// Whole `PlatformInput` events, which the drive builds and the
    /// inspector reads.
    fn events(&self) -> Result<Option<EventLayout>> {
        const ENTRIES: [&str; 12] = [
            "sizes:gpui_platform_input",
            "tags:gpui_input_mouse_move_tag",
            "tags:gpui_input_mouse_down_tag",
            "tags:gpui_input_mouse_up_tag",
            "tags:gpui_input_key_down_tag",
            "tags:gpui_button_right",
            "offsets:gpui_input_mouse_move",
            "offsets:gpui_input_mouse_down",
            "offsets:gpui_input_mouse_up",
            "offsets:gpui_input_key_down",
            "offsets:gpui_mouse_move_pressed_button",
            "offsets:gpui_mouse_event_click_count",
        ];
        self.optional(&ENTRIES, || {
            Ok(EventLayout {
                size: self.size("gpui_platform_input")?,
                tags: (
                    self.tag8("gpui_input_mouse_move_tag")?,
                    self.tag8("gpui_input_mouse_down_tag")?,
                    self.tag8("gpui_input_mouse_up_tag")?,
                ),
                payload: (
                    self.off("gpui_input_mouse_move")?,
                    self.off("gpui_input_mouse_down")?,
                    self.off("gpui_input_mouse_up")?,
                ),
                key_down: (
                    self.tag8("gpui_input_key_down_tag")?,
                    self.off("gpui_input_key_down")?,
                ),
                move_pressed_button: self.off("gpui_mouse_move_pressed_button")?,
                click_count: self.off("gpui_mouse_event_click_count")?,
                button_right: self.tag8("gpui_button_right")?,
            })
        })
    }

    /// Scrolling divs.
    fn scroll(&self, style_size: usize, div_size: usize) -> Result<Option<ScrollLayout>> {
        const ENTRIES: [&str; 6] = [
            "offsets:gpui_div_scroll_offset",
            "offsets:gpui_style_overflow",
            "offsets:gpui_overflow_y",
            "tags:gpui_overflow_scroll",
            "offsets:gpui_rc_refcell_borrow",
            "offsets:gpui_rc_refcell_point",
        ];
        self.optional(&ENTRIES, || {
            let l = ScrollLayout {
                scroll_offset: self.off("gpui_div_scroll_offset")?,
                overflow_y: self.off("gpui_style_overflow")? + self.off("gpui_overflow_y")?,
                overflow_scroll: self.tag8("gpui_overflow_scroll")?,
                rc_borrow: self.off("gpui_rc_refcell_borrow")?,
                rc_point: self.off("gpui_rc_refcell_point")?,
            };
            if l.scroll_offset + 8 > div_size
                || !l.scroll_offset.is_multiple_of(8)
                || l.overflow_y >= style_size
            {
                bail!("the scroll fields run past the Div or its style");
            }
            if l.rc_borrow < 16 || l.rc_point < l.rc_borrow + 8 || l.rc_point + 8 > 64 {
                bail!(
                    "implausible Rc<RefCell<Point>> layout: borrow at {}, point at {}",
                    l.rc_borrow,
                    l.rc_point
                );
            }
            Ok(l)
        })
    }

    /// Painting quads, where the build kept `Window::paint_quad`.
    fn paint(&self) -> Result<Option<PaintLayout>> {
        const ENTRIES: [&str; 3] = [
            "templates:gpui_paint_quad",
            "offsets:gpui_paint_quad_bounds",
            "offsets:gpui_paint_quad_hsla",
        ];
        if self.fns.paint_quad == 0 {
            return Ok(None);
        }
        self.optional(&ENTRIES, || {
            let l = PaintLayout {
                paint_quad: self.fns.paint_quad,
                bounds: self.off("gpui_paint_quad_bounds")?,
                hsla: self.off("gpui_paint_quad_hsla")?,
                template: self.template("gpui_paint_quad")?,
            };
            if l.template.len() > 1024 {
                bail!("the PaintQuad template is {} bytes", l.template.len());
            }
            fits(
                "the PaintQuad template",
                l.template.len(),
                &[(l.bounds, 16), (l.hsla, 16)],
            )?;
            Ok(l)
        })
    }

    /// Drawing images, where the build has `Window::paint_image`.
    fn image(&self) -> Result<Option<ImageLayout>> {
        const ENTRIES: [&str; 8] = [
            "templates:gpui_render_image",
            "offsets:gpui_render_image_id",
            "offsets:gpui_render_image_width",
            "offsets:gpui_render_image_height",
            "offsets:gpui_render_image_pixels_cap",
            "offsets:gpui_render_image_pixels_ptr",
            "offsets:gpui_render_image_pixels_len",
            "offsets:gpui_rc_value",
        ];
        if self.fns.paint_image == 0 {
            return Ok(None);
        }
        self.optional(&ENTRIES, || {
            let l = ImageLayout {
                paint_image: self.fns.paint_image,
                arc_value: self.off("gpui_rc_value")?,
                template: self.template("gpui_render_image")?,
                id: self.off("gpui_render_image_id")?,
                width: self.off("gpui_render_image_width")?,
                height: self.off("gpui_render_image_height")?,
                pixels: [
                    self.off("gpui_render_image_pixels_cap")?,
                    self.off("gpui_render_image_pixels_ptr")?,
                    self.off("gpui_render_image_pixels_len")?,
                ],
            };
            let n = l.template.len();
            if n > 4096 || l.arc_value != 16 || l.pixels.iter().any(|o| o % 8 != 0) {
                bail!("the RenderImage template ({n} bytes) or its Arc is implausible");
            }
            fits(
                "the RenderImage template",
                n,
                &[
                    (l.id, 8),
                    (l.width, 4),
                    (l.height, 4),
                    (l.pixels[0], 8),
                    (l.pixels[1], 8),
                    (l.pixels[2], 8),
                ],
            )?;
            Ok(l)
        })
    }

    /// Actions by name, which only the drive dispatches.
    fn actions(&self) -> Result<Option<ActionLayout>> {
        const ENTRIES: [&str; 10] = [
            "offsets:gpui_app_actions",
            "offsets:gpui_rc_value",
            "offsets:gpui_action_registry_by_name",
            "offsets:gpui_hashmap_ctrl",
            "offsets:gpui_hashmap_bucket_mask",
            "sizes:gpui_action_bucket",
            "offsets:gpui_action_bucket_key",
            "offsets:gpui_action_bucket_value",
            "offsets:gpui_action_data_build",
            "templates:gpui_json_null",
        ];
        self.optional(&ENTRIES, || {
            let l = ActionLayout {
                app_actions: self.off("gpui_app_actions")?,
                rc_value: self.off("gpui_rc_value")?,
                registry_by_name: self.off("gpui_action_registry_by_name")?,
                map_ctrl: self.off("gpui_hashmap_ctrl")?,
                map_bucket_mask: self.off("gpui_hashmap_bucket_mask")?,
                bucket_size: self.size("gpui_action_bucket")?,
                bucket_key: self.off("gpui_action_bucket_key")?,
                bucket_value: self.off("gpui_action_bucket_value")?,
                build: self.off("gpui_action_data_build")?,
                json_null: self.template("gpui_json_null")?,
            };
            fits(
                "an action bucket",
                l.bucket_size,
                &[(l.bucket_key, 16), (l.bucket_value + l.build, 8)],
            )?;
            Ok(l)
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A layout recovered from the mock client, kept as test data.
    pub(crate) fn mock_spec() -> Spec {
        let v = json_lite::Value(include_str!("../testdata/mock-layout.json"));
        let fns = Fns {
            interactivity_new: 1,
            div_into_any_element: 2,
            str_into_any_element: 3,
            dispatch_event: 7,
            window_focus: 8,
            window_blur: 9,
            window_refresh: 10,
            focus_handle_new: 11,
            app_focus_handle: 0,
            focus_handle_clone: 12,
            window_dispatch_action: 14,
            wake_platform: 13,
            paint_quad: 0,
            paint_image: 0,
        };
        spec_from_maps(
            fns,
            5,
            &v.nums("offsets"),
            &v.nums("sizes"),
            &v.nums("tags"),
            &v.strs("templates"),
        )
        .unwrap()
    }

    #[test]
    fn the_committed_layout_resolves() {
        let r = mock_spec().resolve().unwrap();
        assert_eq!(r.div_size, 1360);
        let scroll = r.scroll.expect("the mock's layout has the scroll entries");
        assert_eq!(
            (
                scroll.scroll_offset,
                scroll.overflow_scroll,
                scroll.rc_borrow,
                scroll.rc_point
            ),
            (1080, 3, 16, 24)
        );
        // `overflow` at 624 in the style, `y` its second byte.
        assert_eq!(scroll.overflow_y, 625);
        assert_eq!(r.base_style, 1040);
        assert_eq!(r.children, 1256);
        assert_eq!(
            (
                r.i.mouse_down_listeners,
                r.i.hover_style,
                r.i.hitbox_behavior,
                r.c.mouse_button
            ),
            (408, 1120, 1247, 16)
        );
        assert_eq!(
            (r.t.phase_bubble, r.c.button_left, r.t.hitbox_block_mouse),
            (0, 2, 1)
        );
        assert_eq!(
            (r.i.group, r.i.group_hover_style, r.i.group_style_box),
            (952, 976, 24)
        );
        assert_eq!(
            (
                r.t.visibility_visible,
                r.t.visibility_hidden,
                r.s.visibility
            ),
            (0, 1, 626)
        );
        assert_eq!(r.t.shared_string_static, 0x18);
        assert_eq!(
            (
                r.i.key_down_listeners,
                r.i.tracked_focus_handle,
                r.i.focusable
            ),
            (624, 880, 1240)
        );
        assert_eq!(
            (r.c.scene, r.c.quads, r.c.quad.0, r.c.quad.2),
            (960, 176, 160, 40)
        );
        assert_eq!(
            (
                r.c.next_frame_callbacks,
                r.c.platform_window,
                r.c.schedule_frame_slot
            ),
            (5600, 5544, 408)
        );
        let e = r.c.events.expect("the test layout has whole events");
        assert_eq!((e.size, e.tags), (88, (11, 8, 9)));
        assert_eq!((r.s.font_weight, r.s.mouse_cursor), (48, 639));
        assert_eq!(r.style_size, 640);
        assert_eq!(r.s.background, 336);
        assert_eq!(r.s.text_color, 136);
        assert_eq!(r.s.padding, [408, 416, 424, 432]);
    }

    #[test]
    fn the_drive_only_entries_are_optional() {
        let mut s = mock_spec();
        for name in [
            "gpui_app_actions",
            "gpui_rc_value",
            "gpui_action_registry_by_name",
            "gpui_hashmap_ctrl",
            "gpui_hashmap_bucket_mask",
            "gpui_action_bucket_key",
            "gpui_action_bucket_value",
            "gpui_action_data_build",
            "gpui_input_mouse_move",
            "gpui_input_mouse_down",
            "gpui_input_mouse_up",
            "gpui_input_key_down",
            "gpui_mouse_move_pressed_button",
            "gpui_mouse_event_click_count",
        ] {
            s.offsets.remove(name);
        }
        s.sizes.remove("gpui_action_bucket");
        s.sizes.remove("gpui_platform_input");
        for name in [
            "gpui_input_mouse_move_tag",
            "gpui_input_mouse_down_tag",
            "gpui_input_mouse_up_tag",
            "gpui_input_key_down_tag",
            "gpui_button_right",
        ] {
            s.tags.remove(name);
        }
        s.templates.remove("gpui_json_null");
        let r = s.resolve().unwrap();
        assert!(r.c.events.is_none() && r.c.actions.is_none());
        // Part of a group is a broken layout file, not an absent group.
        let mut s = mock_spec();
        s.offsets.remove("gpui_hashmap_ctrl");
        assert!(s.resolve().unwrap_err().to_string().contains("9 of the 10"));
    }

    #[test]
    fn a_layout_missing_an_entry_is_refused() {
        let mut s = mock_spec();
        s.offsets.remove("gpui_style_background");
        assert!(s.resolve().is_err());
        let mut s = mock_spec();
        s.sizes.insert("gpui_style_refinement".into(), 100);
        assert!(s.resolve().unwrap_err().to_string().contains("runs past"));
    }

    #[test]
    fn a_field_past_its_value_is_refused() {
        // Each of these offsets puts a field rekindle reads or writes past
        // the end of the value it belongs to.
        for (section, name, value) in [
            ("sizes", "gpui_quad", 40),
            ("offsets", "gpui_mono_sprite_color", 200),
            ("offsets", "gpui_children_capacity", 200),
            ("offsets", "gpui_mouse_event_click_count", 88),
            ("offsets", "gpui_action_data_build", 64),
            ("offsets", "gpui_hitbox_id", 64),
        ] {
            let mut s = mock_spec();
            let map = if section == "sizes" {
                &mut s.sizes
            } else {
                &mut s.offsets
            };
            map.insert(name.into(), value);
            let err = s.resolve().expect_err(name).to_string();
            assert!(err.contains("runs past"), "{name}: {err}");
        }
    }

    #[test]
    fn templates_are_pairs_of_hex_digits() {
        assert_eq!(from_hex("00ff7a"), Some(vec![0x00, 0xff, 0x7a]));
        assert_eq!(from_hex(""), Some(vec![]));
        for bad in ["0", "0g", "+f", "é0"] {
            assert_eq!(from_hex(bad), None, "{bad}");
        }
    }

    /// A small JSON reader for the test data (the crate has no serde): the
    /// flat objects under each top-level key.
    mod json_lite {
        use std::collections::BTreeMap;

        pub(super) struct Value(pub(super) &'static str);

        impl Value {
            fn object(&self, key: &str) -> &str {
                let at = self
                    .0
                    .find(&format!("\"{key}\": {{"))
                    .unwrap_or_else(|| panic!("no {key}"));
                let start = self.0[at..].find('{').unwrap() + at + 1;
                let end = self.0[start..].find('}').unwrap() + start;
                &self.0[start..end]
            }

            fn pairs(&self, key: &str) -> Vec<(String, String)> {
                self.object(key)
                    .split(',')
                    .filter_map(|kv| {
                        let (k, v) = kv.split_once(':')?;
                        Some((
                            k.trim().trim_matches('"').to_string(),
                            v.trim().trim_matches('"').to_string(),
                        ))
                    })
                    .collect()
            }

            pub(super) fn nums(&self, key: &str) -> BTreeMap<String, usize> {
                self.pairs(key)
                    .into_iter()
                    .filter_map(|(k, v)| Some((k, v.parse().ok()?)))
                    .collect()
            }

            pub(super) fn strs(&self, key: &str) -> BTreeMap<String, String> {
                self.pairs(key).into_iter().collect()
            }
        }
    }
}
