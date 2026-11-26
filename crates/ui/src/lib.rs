//! One plugin UI, two native backends.
//!
//! A plugin writes a [`Ui`] tree from the semantic vocabulary in [`ui`] and
//! a theme as [`Theme`] tokens. The host lowers the same tree twice:
//!
//! - [`wire`] lowers it to a [`rekindle_wire::Turn`], the op buffer a split
//!   GPUI client applies on Android and iOS.
//! - [`gpui`] lowers it to a [`gpui::GpuiNode`] tree: a description of real
//!   `Div` and text elements, with style fields named the way
//!   `StyleRefinement` names them. The desktop backend builds those elements
//!   with the client's own constructors; nothing here draws.
//!
//! The vocabulary is deliberately small: `row`, `column`, `text`, `button`,
//! `toggle`, `icon`, `field`, `surface` and `separator`. A component is a
//! recipe, not a widget: each backend uses its platform's own controls and
//! layout, so the result is native on both. A plugin never writes wire props
//! or GPUI styles itself.
//!
//! Tokens are names first (the canonical set is [`rekindle_plugin_host`]'s
//! `tokens` module); this crate only needs the name, the value and the
//! fallback, so it carries a small [`theme::Tokens`] table rather than a
//! dependency on the host runtime.

pub mod gpui;
pub mod theme;
pub mod ui;
pub mod wire;

pub use gpui::{GpuiAxis, GpuiDiv, GpuiIcon, GpuiLen, GpuiNode, GpuiStyle, GpuiText};
pub use theme::{Brush, Color, Theme, Token, Tokens};
pub use ui::{Align, Axis, ButtonKind, Justify, Length, Node, Padding, TextStyle, Ui};
