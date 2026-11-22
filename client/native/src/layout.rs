//! The recovered layout, typed: where the client's gpui keeps each field
//! rekindle reads or writes, and how its enums are encoded. [`Resolved`] is
//! built and checked once by `Spec::resolve`; nothing here reads memory.

use crate::spec::Fns;

/// Everything rekindle needs from one client build, checked: every offset
/// it writes at lies inside the value it writes.
#[derive(Debug)]
pub(crate) struct Resolved {
    pub fns: Fns,
    /// An address in the client's code: a window root's `render` pointer
    /// must lie close to it, or it isn't a render fn.
    pub near: usize,
    /// `Window.root` (`Option<AnyView>`) and `AnyView.render` in it.
    pub window_root: usize,
    pub view_render: usize,
    pub div_size: usize,
    /// `Div.interactivity.base_style`, a `Box<StyleRefinement>`.
    pub base_style: usize,
    /// `Div.children`, a `SmallVec<[AnyElement; 2]>`.
    pub children: usize,
    pub i: Interaction,
    pub c: ClientLayout,
    pub style_size: usize,
    pub s: StyleOffsets,
    pub t: Tags,
    /// `Some(Fill)` as gpui builds it, and where its colour is: what a
    /// background is written from.
    pub fill: Vec<u8>,
    pub scroll: Option<ScrollLayout>,
    pub paint: Option<PaintLayout>,
    pub image: Option<ImageLayout>,
}

/// Offsets of the style fields rekindle writes, from the start of a
/// `StyleRefinement`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StyleOffsets {
    pub text_color: usize,
    pub text_size: usize,
    pub text_line_height: usize,
    pub background: usize,
    /// top, right, bottom, left.
    pub border_widths: [usize; 4],
    pub border_color: usize,
    /// top, right, bottom, left.
    pub padding: [usize; 4],
    pub inset: [usize; 4],
    /// `top_left`, `top_right`, `bottom_right`, `bottom_left`.
    pub corners: [usize; 4],
    pub width: usize,
    pub height: usize,
    pub gap: usize,
    pub position: usize,
    pub display: usize,
    pub visibility: usize,
    pub flex_direction: usize,
    pub align_items: usize,
    pub justify_content: usize,
    pub font_weight: usize,
    pub mouse_cursor: usize,
}

/// Enum encodings, as gpui itself lays them out (measured, not assumed).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tags {
    pub len_pixels: u32,
    pub len_fraction: u32,
    pub len_auto: u32,
    pub option_some: u32,
    pub position_absolute: u8,
    pub display_flex: u8,
    pub flex_row: u8,
    pub flex_column: u8,
    pub align_start: u8,
    pub align_end: u8,
    pub align_center: u8,
    pub phase_bubble: u8,
    pub hitbox_block_mouse: u8,
    pub font_weight_some: u32,
    pub cursor_text: u8,
    pub cursor_pointer: u8,
    pub visibility_visible: u8,
    pub visibility_hidden: u8,
    pub shared_string_static: u8,
}

/// Where a Div keeps what makes it interactive (all `pub(crate)` in gpui).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Interaction {
    /// `Vec<Box<dyn Fn(&MouseDownEvent, DispatchPhase, &Hitbox, &mut Window, &mut App)>>`.
    pub mouse_down_listeners: usize,
    /// `Option<Box<StyleRefinement>>`.
    pub hover_style: usize,
    /// `HitboxBehavior`, one byte.
    pub hitbox_behavior: usize,
    /// `Option<SharedString>`.
    pub group: usize,
    /// `Option<GroupStyle>`: the group name, then its `Box<StyleRefinement>`.
    pub group_hover_style: usize,
    pub group_style_box: usize,
    /// A static `SharedString`'s `&str`, after its tag byte.
    pub shared_string_ptr: usize,
    pub shared_string_len: usize,
    /// `Vec<Box<dyn Fn(&KeyDownEvent, DispatchPhase, &mut Window, &mut App)>>`.
    pub key_down_listeners: usize,
    /// `Option<FocusHandle>` and the `focusable` byte `track_focus` sets.
    pub tracked_focus_handle: usize,
    pub focusable: usize,
}

/// What rekindle needs to paint quads itself (`crate::paint`).
#[derive(Clone, Debug)]
pub(crate) struct PaintLayout {
    /// `Window::paint_quad`.
    pub paint_quad: usize,
    /// A `PaintQuad` as `gpui::fill` builds it, and where its bounds and its
    /// background's colour sit.
    pub template: Vec<u8>,
    pub bounds: usize,
    pub hsla: usize,
}

/// What rekindle needs to draw images itself: `Window::paint_image`, and
/// an `Arc<RenderImage>` of one frame, built from a template at measured
/// offsets.
#[derive(Clone, Debug)]
pub(crate) struct ImageLayout {
    pub paint_image: usize,
    /// The value in an `Arc`'s allocation (after the two counts).
    pub arc_value: usize,
    /// A `RenderImage` gpui built, and where its id, its frame's width and
    /// height (u32) and its pixel `Vec`'s words sit.
    pub template: Vec<u8>,
    pub id: usize,
    pub width: usize,
    pub height: usize,
    pub pixels: [usize; 3],
}

/// What a div needs to scroll as `overflow_y_scroll()` makes it (all
/// measured).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollLayout {
    /// `Interactivity.scroll_offset`: `Option<Rc<RefCell<Point<Pixels>>>>`.
    /// gpui takes it from element state only for an element with an id, and
    /// rekindle's have none, so a value written here is the one it uses.
    pub scroll_offset: usize,
    /// `StyleRefinement.overflow.y`: `Option<Overflow>`, one byte.
    pub overflow_y: usize,
    pub overflow_scroll: u8,
    /// In the `Rc`'s allocation: the borrow flag and the point.
    pub rc_borrow: usize,
    pub rc_point: usize,
}

/// Where the client's window keeps what rekindle reads, and the frame
/// record's layout.
#[derive(Clone, Debug)]
pub(crate) struct ClientLayout {
    pub viewport_size: usize,
    pub scale_factor: usize,
    pub mouse_position: usize,
    pub focus: usize,
    pub hitboxes: usize,
    pub scene: usize,
    /// The same in `rendered_frame`: where the frame just drawn is once
    /// `Window::draw` has returned (`client::read_rendered_frame`).
    pub rendered_hitboxes: usize,
    pub rendered_scene: usize,
    pub next_frame_callbacks: usize,
    pub refcell_borrow: usize,
    pub refcell_vec: usize,
    pub platform_window: usize,
    pub schedule_frame_slot: usize,
    pub invalidator: usize,
    pub propagate_event: usize,
    pub focus_handles: usize,
    pub focus_handle_size: usize,
    /// Scene Vec offsets.
    pub quads: usize,
    pub shadows: usize,
    pub underlines: usize,
    pub mono: usize,
    pub subpx: usize,
    /// A quad's size, and its bounds, background and border colour.
    pub quad: (usize, usize, usize, usize),
    /// (size, colour offset) of each primitive.
    pub shadow: (usize, usize),
    pub underline: (usize, usize),
    pub mono_sprite: (usize, usize),
    pub subpx_sprite: (usize, usize),
    pub background_solid_tag: u32,
    /// The colour in a `Background` (a quad's, or a `Some(Fill)`'s payload).
    pub fill_hsla: usize,
    /// A hitbox's size and its bounds.
    pub hitbox: (usize, usize),
    /// `MouseDownEvent.button` (and `MouseUpEvent`'s), and the left
    /// button's tag.
    pub mouse_button: usize,
    pub button_left: u8,
    /// Whole `PlatformInput` events, for building and reading them. Only
    /// the drive and the inspector need this; `None` where the layout file
    /// lacks it.
    pub events: Option<EventLayout>,
    /// The focused field's input handler, for inserting text. `None` where
    /// the layout file lacks it.
    pub input: Option<InputLayout>,
    /// `bounds` and `order` of a monochrome / subpixel sprite (a text glyph).
    pub sprite_bounds: (usize, usize),
    pub sprite_order: (usize, usize),
    /// The scene's images (`polychrome_sprites`): where the list is, a
    /// sprite's size and its bounds. Optional: older layouts don't have it.
    pub poly: Option<(usize, usize, usize)>,
    /// In a polychrome sprite: its atlas tile, the tile's bounds (device
    /// pixels, i32) and padding (u32). Optional.
    pub poly_tile: Option<(usize, usize, usize)>,
    /// A quad's `order` (gpui's `DrawOrder`: what is painted over what).
    pub quad_order: usize,
    pub key: KeyLayout,
    /// For `hitbox_hovered`: `Hitbox.id`, the window's captured hitbox
    /// (`Option<HitboxId>`: u32 tag, id at +8), last input modality (one byte)
    /// and mouse hit test (`HitTest`: ids `SmallVec<[HitboxId; 8]>`, then the
    /// hover count).
    pub hitbox_id: usize,
    pub captured_hitbox: usize,
    pub option_some_tag: u32,
    pub last_input_modality: usize,
    pub modality_keyboard: u8,
    pub mouse_hit_test: usize,
    pub hit_test_ids: usize,
    pub ids_capacity: usize,
    pub ids_inline: usize,
    pub hit_test_hover_count: usize,
    /// The Div's children `SmallVec<[AnyElement; 2]>`: capacity field offset
    /// and inline capacity (the element size is an `AnyElement`'s, 24).
    pub children_capacity: usize,
    pub children_inline: usize,
    /// Actions by name. Only the drive needs this; `None` where the layout
    /// file lacks it.
    pub actions: Option<ActionLayout>,
    /// `AnyView.view_type_name` (a `&'static str`).
    pub view_type_name: usize,
    pub window_root: usize,
    /// A `Vec`'s three words: where `cap`, `ptr` and `len` sit.
    pub vec: [usize; 3],
}

/// Text into the focused field the way the platform delivers a typed
/// character: the platform window's `take_input_handler` and
/// `set_input_handler` vtable slots, the `Option<PlatformInputHandler>` they
/// move (its size; the first word is 0 for `None`), the `Box<dyn
/// InputHandler>` inside it, and `replace_text_in_range`'s slot in that box's
/// vtable.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InputLayout {
    pub take_slot: usize,
    pub set_slot: usize,
    pub size: usize,
    pub handler: usize,
    /// `Option<FocusId>` of the field that registered the handler.
    pub focus_id: usize,
    pub replace_slot: usize,
}

/// A `PlatformInput` as a whole: its size, the tags and payload offsets of
/// the mouse move, down and up variants and of key down, and the mouse
/// event fields a built event needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EventLayout {
    pub size: usize,
    pub tags: (u8, u8, u8),
    pub payload: (usize, usize, usize),
    pub key_down: (u8, usize),
    pub move_pressed_button: usize,
    pub click_count: usize,
    pub button_right: u8,
}

/// Actions by name: `App.actions` (an `Rc<ActionRegistry>`, value at
/// `rc_value`), `ActionRegistry.by_name` (a std `HashMap<&str, ActionData>`
/// with a zero-sized hasher: its hashbrown table's ctrl pointer and bucket
/// mask), each bucket a `(&str, ActionData)` tuple, and `ActionData.build`.
#[derive(Clone, Debug)]
pub(crate) struct ActionLayout {
    pub app_actions: usize,
    pub rc_value: usize,
    pub registry_by_name: usize,
    pub map_ctrl: usize,
    pub map_bucket_mask: usize,
    pub bucket_size: usize,
    pub bucket_key: usize,
    pub bucket_value: usize,
    pub build: usize,
    /// `serde_json::Value::Null`'s bytes (what a parameterless action builds
    /// from).
    pub json_null: Vec<u8>,
}

/// A `Keystroke` (its `key` and `key_char` strings and its `Modifiers`
/// bytes), and a `String`'s three words.
#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyLayout {
    pub key: usize,
    pub key_char: usize,
    pub modifiers: usize,
    pub control: usize,
    pub alt: usize,
    pub shift: usize,
    pub platform: usize,
    pub string: [usize; 3],
}
