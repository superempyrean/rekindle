//! `PropId`: the u16 property key, recovered in full from Phoenix's host.
//!
//! RECOVERED (byte-for-byte from `liblibdiscord_wasm_host.so`): the 855 ids,
//! their names, and the two-table layout (a dense id->name array the host
//! indexes directly, and a name-sorted array it binary-searches). The numbers
//! and names here are copied from `propid-names.tsv`; the generated tables live
//! in `prop_id_table.rs` (see `build.rs`).
//!
//! NOT recovered (and not claimed here): each id's value *type* (which [`Value`]
//! tag it carries) and which element kind honours it. The id space is untyped
//! at this layer. That pairing lives in the guest `gpui` crate, which the
//! Android build (356207) does not ship.
//!
//! [`Value`]: crate::Value

include!(concat!(env!("OUT_DIR"), "/prop_id_table.rs"));

/// A styleable / animatable property of an element, addressed by a stable u16.
///
/// Ids are numbered in append-only alphabetical batches (a release appends its
/// new props at the end), so the numeric id is the durable key and the name is
/// its label. Valid ids are `0..`[`PropId::COUNT`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PropId(pub u16);

impl PropId {
    /// The number of assigned ids: 855 (ids 0..=854), the bound the host checks.
    pub const COUNT: u16 = COUNT as u16;

    /// The id's recovered name, or `None` if the id is out of range.
    pub fn name(self) -> Option<&'static str> {
        NAMES.get(self.0 as usize).copied()
    }

    /// The id for a name, found the way the host does: a binary search of the
    /// name-sorted table. `None` if no property has that name.
    pub fn from_name(name: &str) -> Option<PropId> {
        SORTED
            .binary_search_by(|(n, _)| (*n).cmp(name))
            .ok()
            .map(|i| PropId(SORTED[i].1))
    }

    /// Whether the id is within the recovered range.
    pub fn is_valid(self) -> bool {
        (self.0 as usize) < NAMES.len()
    }
}

impl core::fmt::Debug for PropId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.name() {
            Some(name) => write!(f, "PropId({}, {name:?})", self.0),
            None => write!(f, "PropId({}, ?)", self.0),
        }
    }
}

/// The recolour ids rekindle writes, observed and stable in the host binary,
/// where recolouring a view is writing these cells. Every one `gpui-native`
/// has a setter for holds an integer colour, `0xAARRGGBB`. The gradients
/// hold arrays, and the guest's app code writes the `active*` mirrors.
pub mod ids {
    use super::PropId;

    pub const BACKGROUND: PropId = PropId(90);
    pub const BACKGROUND_GRADIENT: PropId = PropId(91);
    pub const COLOR: PropId = PropId(129);
    pub const TINT: PropId = PropId(522);
    pub const BORDER_COLOR: PropId = PropId(104);
    pub const SCRIM: PropId = PropId(432);
    pub const SCRIM_COLOR: PropId = PropId(433);
    pub const TEXT_GRADIENT: PropId = PropId(503);
    pub const PLACEHOLDER_COLOR: PropId = PropId(355);
    pub const SELECTION_COLOR: PropId = PropId(844);
    pub const HANDLE_COLOR: PropId = PropId(229);
    pub const ACTIVE_BACKGROUND: PropId = PropId(21);
    pub const ACTIVE_BORDER_COLOR: PropId = PropId(25);
    pub const FOCUS_BORDER_COLOR: PropId = PropId(201);
    pub const ELEVATION_COLOR: PropId = PropId(760);
    pub const NAV_SCRIM_COLOR: PropId = PropId(813);
}
