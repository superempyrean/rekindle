//! The client's native message context menu, resolved from the build.
//!
//! The menu is a `Vec<MenuItem>` returned by `MessageContextMenu`'s item
//! builder. Each `MenuItem` is an enum: tag 2 is an `Action`, tag 4 a
//! `Separator`. An `Action` carries a `MenuItemProps` at `props`; the visible
//! label is a `SharedString` at `props + label`, whose static form is
//! `(tag 0x8018, pointer, length)`.
//!
//! Everything is recovered by shape:
//! - `build_fn` from the `MessageContextMenu` string and its `lea`;
//! - `vec_push` from the call after the `Copy Message ID` label block;
//! - `item_size` from `Vec::push`'s `imul reg, reg, imm32` stride;
//! - `props` from the `rep movsb` in `MenuItem`'s core constructor;
//! - `label` from `MenuItemProps::new`'s two stores of the label
//!   (`movups [rax+disp], xmm0` and `mov [rax+disp+0x10], rdx`).

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::Image;

const MENU_STRING: &[u8] = b"MessageContextMenu";
const COPY_ID: &[u8] = b"Copy Message ID";
const ROW_PROLOGUE: &[u8] = &[
    0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53,
];

/// A recovered `MenuItem` layout.
#[derive(Clone, Debug, Serialize)]
pub struct Menu {
    pub build_fn: u64,
    pub vec_push: u64,
    /// `MenuItemProps::new`, `with_id`, `with_icon`, `with_color`.
    pub props_new: u64,
    pub with_id: u64,
    pub with_icon: u64,
    pub with_color: u64,
    pub item_size: usize,
    /// `MenuItemProps` inside a `MenuItem`.
    pub props: usize,
    pub props_size: usize,
    /// The visible `SharedString` inside `MenuItemProps`.
    pub label: Label,
    pub action_tag: u64,
    /// The tag observed on separator entries in the builder's own list.
    pub separator_tag: u64,
    /// An action's click handler, `Rc<dyn Fn(&ClickEvent, &mut Window,
    /// &mut App)>`, and the word before it the core constructor zeroes.
    pub on_click: usize,
    pub on_click_tag: usize,
    /// The message's id and its channel's in the builder's input, the
    /// second argument.
    pub message_id: usize,
    pub channel_id: usize,
    /// The colour Delete Message passes `with_color`: the danger colour.
    pub danger: u8,
    /// The icons Copy Message ID and Delete Message pass `with_icon`.
    pub id_icon: u16,
    pub trash_icon: u16,
}

/// A `SharedString`'s fields, relative to the item.
#[derive(Clone, Debug, Serialize)]
pub struct Label {
    pub tag: usize,
    pub ptr: usize,
    pub len: usize,
}

impl Menu {
    /// Resolve the menu for `image`.
    pub fn resolve(image: &Image<'_>) -> Result<Self> {
        let build_fn = build_fn(image)?;
        let (start, end) = image
            .function_named("MessageContextMenu", build_fn)
            .context("MessageContextMenu has no extent")?;
        let copy_lea = lea_in_function(image, start, end, COPY_ID)
            .context("Copy Message ID lea not found in MessageContextMenu")?;
        let calls = calls_after(image, copy_lea, 0x200);
        if calls.len() < 5 {
            bail!("Copy Message ID block has only {} calls", calls.len());
        }
        let vec_push = calls[4];
        let with_id = calls[2];
        let with_icon = calls[3];
        let core = core_ctor(image, calls[1])?;
        let (props, props_size, props_new) = props_layout(image, core)?;
        let (label_tag, label_ptr, label_len) = label_layout(image, props_new)?;
        let item_size = push_stride(image, vec_push)?;
        if item_size < props + props_size {
            bail!("MenuItem is {item_size:#x} but props ({props:#x}+{props_size:#x}) does not fit");
        }
        let label = Label {
            tag: props + label_tag,
            ptr: props + label_ptr,
            len: props + label_len,
        };
        for (name, at) in [
            ("label tag", label.tag),
            ("label ptr", label.ptr),
            ("label len", label.len),
        ] {
            if at + 8 > item_size {
                bail!("{name} at {at:#x} is outside the {item_size:#x}-byte item");
            }
        }
        let action_tag = action_tag(image, core)?;
        let (on_click_tag, on_click) = handler(image, core)?;
        let (message_id, channel_id) = ids(image, build_fn)?;
        let with_color = with_color(image, copy_lea)?;
        let id_icon = immediate_before(image, copy_lea, 3, Register::R8W)
            .context("Copy Message ID's icon")?;
        let delete_lea = lea_in_function(image, start, end, b"Delete Message")
            .context("Delete Message lea not found in MessageContextMenu")?;
        let trash_icon = immediate_before(image, delete_lea, 3, Register::R8W)
            .context("Delete Message's icon")?;
        let danger = immediate_before(image, delete_lea, 4, Register::R8L)
            .context("Delete Message's colour")?;
        Ok(Self {
            build_fn,
            vec_push,
            props_new,
            with_id,
            with_icon,
            with_color,
            item_size,
            props,
            props_size,
            label,
            action_tag,
            separator_tag: 4,
            on_click,
            on_click_tag,
            message_id,
            channel_id,
            danger: u8::try_from(danger).context("a colour over 255")?,
            id_icon: u16::try_from(id_icon).context("an icon over 0xffff")?,
            trash_icon: u16::try_from(trash_icon).context("an icon over 0xffff")?,
        })
    }
}

/// The immediate the last `mov reg, imm` puts in `reg` before the `nth`
/// call (from 0) after `from`.
fn immediate_before(image: &Image<'_>, from: u64, nth: usize, reg: Register) -> Option<u64> {
    let code = image.read(from, 0x200)?;
    let mut value = None;
    let mut calls = 0;
    for insn in Decoder::with_ip(64, code, from, DecoderOptions::NONE) {
        if insn.mnemonic() == Mnemonic::Call {
            if calls == nth {
                return value;
            }
            calls += 1;
            value = None;
        }
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Register
            && insn.op0_register() == reg
        {
            value = match insn.op1_kind() {
                OpKind::Immediate8 => Some(u64::from(insn.immediate8())),
                OpKind::Immediate16 => Some(u64::from(insn.immediate16())),
                OpKind::Immediate32 => Some(u64::from(insn.immediate32())),
                _ => None,
            };
        }
    }
    None
}

fn build_fn(image: &Image<'_>) -> Result<u64> {
    let strings = image
        .find(MENU_STRING)
        .into_iter()
        .filter(|&va| {
            let before = image.read(va.wrapping_sub(1), 1).map_or(0, |b| b[0]);
            before == 0 || !before.is_ascii_alphanumeric()
        })
        .collect::<Vec<_>>();
    if strings.is_empty() {
        bail!("no MessageContextMenu string");
    }
    let mut found = Vec::new();
    for string in strings {
        for lea in image.leas_to(string, string + 1) {
            if let Some((start, _)) = image.function(lea)
                && image
                    .read(start, ROW_PROLOGUE.len())
                    .is_some_and(|p| p == ROW_PROLOGUE)
            {
                found.push(start);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no eight-push function loads MessageContextMenu"),
        many => bail!(
            "{} candidate MessageContextMenu functions: {many:x?}",
            many.len()
        ),
    }
}

fn lea_in_function(image: &Image<'_>, start: u64, end: u64, needle: &[u8]) -> Option<u64> {
    let targets = image.find(needle);
    if targets.is_empty() {
        return None;
    }
    let len = usize::try_from(end - start).ok()?;
    let code = image.read(start, len)?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Lea
            && insn.op1_kind() == OpKind::Memory
            && insn.memory_base() == Register::RIP
            && targets.contains(&insn.ip_rel_memory_address())
        {
            return Some(insn.ip());
        }
    }
    None
}

fn calls_after(image: &Image<'_>, start: u64, len: usize) -> Vec<u64> {
    let Some(code) = image.read(start, len) else {
        return Vec::new();
    };
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut out = Vec::new();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Call
            && let Some(target) = branch_target(&insn)
        {
            out.push(target);
        }
    }
    out
}

fn branch_target(insn: &Instruction) -> Option<u64> {
    match insn.op0_kind() {
        OpKind::NearBranch64 | OpKind::NearBranch32 => Some(insn.near_branch64()),
        _ => None,
    }
}

fn last_call(image: &Image<'_>, start: u64, len: usize) -> Option<u64> {
    calls_after(image, start, len).last().copied()
}

fn core_ctor(image: &Image<'_>, copy_ctor: u64) -> Result<u64> {
    let wrapper =
        last_call(image, copy_ctor, 0x80).context("MenuItem copy constructor has no inner call")?;
    last_call(image, wrapper, 0x100).context("MenuItem wrapper has no core call")
}

fn props_layout(image: &Image<'_>, core: u64) -> Result<(usize, usize, u64)> {
    let (start, end) = image.function_named("MenuItem core ctor", core)?;
    let len = usize::try_from(end - start)?;
    let code = image
        .read(start, len)
        .context("MenuItem core ctor is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut props = None;
    let mut props_size = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Lea
            && insn.op0_kind() == OpKind::Register
            && insn.op1_kind() == OpKind::Memory
            && insn.memory_displacement64() >= 0x20
            && insn.memory_displacement64() < 0x80
        {
            props = Some(insn.memory_displacement64() as usize);
        }
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Register
            && insn.op0_register() == Register::ECX
            && insn.op1_kind() == OpKind::Immediate32
        {
            props_size = Some(insn.immediate32() as usize);
        }
    }
    let props_new = calls_after(image, core, 0x80)
        .first()
        .copied()
        .context("MenuItem core ctor has no props_new call")?;
    match (props, props_size) {
        (Some(props), Some(size)) if size > 0 => Ok((props, size, props_new)),
        _ => bail!("MenuItem props layout not found"),
    }
}

fn label_layout(image: &Image<'_>, props_new: u64) -> Result<(usize, usize, usize)> {
    let (start, end) = image
        .function(props_new)
        .unwrap_or((props_new, props_new + 0x200));
    let len = usize::try_from(end - start)?;
    let code = image
        .read(start, len)
        .context("MenuItemProps::new is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut tag = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Movups
            && insn.op0_kind() == OpKind::Memory
            && insn.op1_kind() == OpKind::Register
            && insn.op1_register() == Register::XMM0
        {
            let disp = insn.memory_displacement64() as usize;
            if (0x80..0x100).contains(&disp) {
                tag = Some(disp);
            }
        }
    }
    let tag = tag.context("MenuItemProps label store not found")?;
    Ok((tag, tag + 8, tag + 16))
}

fn with_color(image: &Image<'_>, copy_lea: u64) -> Result<u64> {
    let (start, end) = image.function_named("MessageContextMenu", copy_lea)?;
    let delete_lea = lea_in_function(image, start, end, b"Delete Message")
        .context("Delete Message lea not found in MessageContextMenu")?;
    calls_after(image, delete_lea, 0x150)
        .get(4)
        .copied()
        .context("Delete Message block has no with_color call")
}

fn push_stride(image: &Image<'_>, vec_push: u64) -> Result<usize> {
    let (start, end) = image.function_named("Vec<MenuItem>::push", vec_push)?;
    let len = usize::try_from(end - start)?;
    let code = image.read(start, len).context("Vec push is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Imul {
            for i in 0..insn.op_count() {
                match insn.op_kind(i) {
                    OpKind::Immediate32 => return Ok(insn.immediate32() as usize),
                    OpKind::Immediate32to64 => return Ok(insn.immediate32to64() as usize),
                    OpKind::Immediate8to32 => return Ok(insn.immediate8to32() as usize),
                    _ => {}
                }
            }
        }
    }
    bail!("Vec<MenuItem>::push has no stride")
}

fn action_tag(image: &Image<'_>, core: u64) -> Result<u64> {
    let (start, end) = image.function_named("MenuItem core ctor", core)?;
    let len = usize::try_from(end - start)?;
    let code = image.read(start, len).context("core ctor is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Memory
            && insn.memory_displacement64() == 0
        {
            let tag = match insn.op1_kind() {
                OpKind::Immediate32 => u64::from(insn.immediate32()),
                OpKind::Immediate32to64 => insn.immediate32to64() as u64,
                OpKind::Immediate8to32 => insn.immediate8to32() as u64,
                _ => continue,
            };
            if tag < 16 {
                return Ok(tag);
            }
        }
    }
    bail!("MenuItem action tag not found")
}

/// The core constructor's handler fields: it clears the word before the
/// handler (`and qword [r + tag], 0`) and stores the `Rc` and its vtable.
fn handler(image: &Image<'_>, core: u64) -> Result<(usize, usize)> {
    let (start, end) = image.function_named("MenuItem core ctor", core)?;
    let code = image
        .read(start, usize::try_from(end - start)?)
        .context("core ctor is unmapped")?;
    let insns: Vec<Instruction> = Decoder::with_ip(64, code, start, DecoderOptions::NONE)
        .into_iter()
        .collect();
    let rc = crate::components::rc_field(&insns).context("no handler store in the core ctor")?;
    let tag = insns
        .iter()
        .find(|i| {
            i.mnemonic() == Mnemonic::And
                && i.op0_kind() == OpKind::Memory
                && i.memory_size().size() == 8
                && matches!(
                    i.op1_kind(),
                    OpKind::Immediate8to64 | OpKind::Immediate32to64
                )
                && i.immediate(1) == 0
        })
        .map(|i| i.memory_displacement64() as usize)
        .context("no cleared word before the handler")?;
    if tag + 8 != rc {
        bail!("the cleared word +{tag:#x} isn't before the handler +{rc:#x}");
    }
    Ok((tag, rc))
}

/// The message's id and its channel's in the builder's input: the first
/// two loads from the input, `mov r, [input + at]`, once the builder has
/// kept it in `rax`. The copy-id row passes the first to its action.
fn ids(image: &Image<'_>, build_fn: u64) -> Result<(usize, usize)> {
    let code = image
        .read(build_fn, 0x100)
        .context("MessageContextMenu's head is unmapped")?;
    let loads: Vec<usize> = Decoder::with_ip(64, code, build_fn, DecoderOptions::NONE)
        .into_iter()
        .filter(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Register
                && i.op0_register() != Register::RAX
                && i.op1_kind() == OpKind::Memory
                && i.memory_base() == Register::RAX
                && (0x20..0x200).contains(&i.memory_displacement64())
        })
        .map(|i| i.memory_displacement64() as usize)
        .collect();
    match loads[..] {
        [id, channel, ..] if id.abs_diff(channel) == 8 => Ok((id, channel)),
        _ => bail!("MessageContextMenu's input loads {loads:x?}, not two ids side by side"),
    }
}

/// What a context menu is opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Target {
    User,
    Channel,
    Guild,
}

impl Target {
    /// Strings only that menu's builder loads, all of them: type names for
    /// the user menu, which every user menu variant shares, and i18n keys of
    /// its rows for the others.
    fn strings(self) -> &'static [&'static [u8]] {
        match self {
            Target::User => &[b"ThreadUserContextMenu", b"GroupDMUserContextMenu"],
            Target::Channel => &[b"COLLAPSE_CATEGORY", b"COPY_ID_CHANNEL", b"MARK_AS_READ"],
            Target::Guild => &[b"LEAVE_SERVER", b"INVITE_TO_SERVER", b"CREATE_CATEGORY"],
        }
    }
}

/// The builder of the user, channel or server menu. Like
/// `MessageContextMenu`'s, it returns the rows as a `Vec<MenuItem>` in its
/// first argument. Unlike it, it takes the id of what the menu is opened on
/// by value in its second.
#[derive(Clone, Debug, Serialize)]
pub struct Builder {
    pub target: Target,
    pub build_fn: u64,
    /// The `Vec<MenuItem>::push` it calls most.
    pub vec_push: u64,
}

impl Builder {
    /// The builder for `target` in `image`, whose `MenuItem` is `item_size`
    /// bytes, as [`Menu::resolve`] found it.
    ///
    /// It is the one function with eight pushes in its prologue that loads
    /// each of [`Target::strings`]. It keeps its second argument from its
    /// first instructions (`mov reg, rdx`), and its pushes copy items of
    /// `item_size` bytes.
    pub fn resolve(image: &Image<'_>, target: Target, item_size: usize) -> Result<Self> {
        let wanted = target.strings();
        let mut string_of = std::collections::BTreeMap::new();
        for (i, s) in wanted.iter().enumerate() {
            for at in image.find(s) {
                string_of.insert(at, i);
            }
        }
        let targets: Vec<u64> = string_of.keys().copied().collect();
        let mut sets = vec![std::collections::BTreeSet::new(); wanted.len()];
        for (lea, to) in image.leas_into(&targets) {
            if let Some((start, _)) = image.function(lea)
                && image
                    .read(start, ROW_PROLOGUE.len())
                    .is_some_and(|p| p == ROW_PROLOGUE)
            {
                sets[string_of[&to]].insert(start);
            }
        }
        let mut found = sets.pop().unwrap_or_default();
        for s in &sets {
            found.retain(|f| s.contains(f));
        }
        let build_fn = match found.iter().copied().collect::<Vec<_>>()[..] {
            [one] => one,
            [] => bail!("no {target:?} menu builder loads {:?}", strings(target)),
            ref many => bail!("{} {target:?} menu builders: {many:x?}", many.len()),
        };
        if !keeps_rdx(image, build_fn) {
            bail!("the {target:?} menu builder {build_fn:#x} doesn't keep its second argument");
        }
        let vec_push = most_called_push(image, build_fn, item_size)?;
        Ok(Self {
            target,
            build_fn,
            vec_push,
        })
    }
}

/// The client's icon for each icon of the plugin contract, by the names in
/// the client's icon table. The contract's `note` has no notepad in the
/// client's set, so it is a file.
pub const CONTRACT_ICONS: &[(&str, &str)] = &[
    ("star", "Star"),
    ("bookmark", "Bookmark"),
    ("note", "File"),
    ("palette", "PaintPalette"),
    ("bolt", "Flash"),
    ("heart", "Heart"),
    ("bell", "Bell"),
    ("gear", "Settings"),
    ("link", "Link"),
    ("search", "MagnifyingGlass"),
    ("sparkle", "Sparkles"),
    ("eye", "Eye"),
    ("eye-off", "EyeSlash"),
    ("clock", "Clock"),
    ("pin", "Pin"),
    ("puzzle", "PuzzlePiece"),
    ("translate", "Language"),
    ("microphone", "Microphone"),
    ("headphones", "Headphones"),
    ("camera", "Camera"),
    ("shield", "Shield"),
    ("lock", "Lock"),
    ("user", "User"),
    ("users", "Group"),
    ("chat", "Chat"),
    ("reply", "ArrowAngleLeftUp"),
    ("copy", "Copy"),
    ("trash", "Trash"),
    ("edit", "Pencil"),
    ("check", "CheckmarkLarge"),
    ("cross", "XSmall"),
    ("info", "CircleInformation"),
    ("warning", "Warning"),
    ("download", "Download"),
    ("upload", "Upload"),
    ("globe", "GlobeEarth"),
    ("code", "AngleBrackets"),
    ("hash", "Hashmark"),
    ("at", "At"),
    ("smile", "Reaction"),
    ("image", "Image"),
    ("music", "Music"),
    ("play", "Play"),
    ("pause", "Pause"),
];

/// The icon a menu row passes `with_icon`, for each contract icon name.
///
/// The client's icons are the indices of its icon table, an array of
/// records sorted by name, each holding the name as a `&str`. The table is
/// found from the records of `CircleInformation`, and checked against the
/// client's own rows: Copy Message ID's icon is `Id` and Delete Message's
/// is `Trash`.
pub fn icons(image: &Image<'_>, menu: &Menu) -> Result<std::collections::BTreeMap<String, u16>> {
    let names = icon_names(image)?;
    let index = |name: &str| names.iter().position(|n| n == name);
    for (name, used) in [("Id", menu.id_icon), ("Trash", menu.trash_icon)] {
        if index(name) != Some(usize::from(used)) {
            bail!(
                "the client's rows use icon {used:#x} for {name}, the table has it at {:?}",
                index(name)
            );
        }
    }
    let mut out = std::collections::BTreeMap::new();
    for (contract, client) in CONTRACT_ICONS {
        let i = index(client).with_context(|| format!("no {client} icon for {contract}"))?;
        out.insert((*contract).to_string(), u16::try_from(i)?);
    }
    Ok(out)
}

/// The names in the client's icon table, in order.
fn icon_names(image: &Image<'_>) -> Result<Vec<String>> {
    const ANCHOR: &str = "CircleInformation";
    let name_at = |field: u64| -> Option<String> {
        let ptr = image.qword(field)?;
        let len = usize::try_from(image.qword(field + 8)?).ok()?;
        if !(1..64).contains(&len) {
            return None;
        }
        let bytes = image.read(ptr, len)?;
        bytes
            .iter()
            .all(u8::is_ascii_alphanumeric)
            .then(|| String::from_utf8_lossy(bytes).into_owned())
    };
    let mut tables = Vec::new();
    for at in image.find(ANCHOR.as_bytes()) {
        for field in image.find(&at.to_le_bytes()) {
            if name_at(field).as_deref() != Some(ANCHOR) {
                continue;
            }
            // The stride: the nearest record after it whose name sorts after.
            let Some(stride) = (0x10..0x100).step_by(8).find(|&s| {
                matches!((name_at(field + s), name_at(field + 2 * s)),
                    (Some(a), Some(b)) if a.as_str() > ANCHOR && b > a)
            }) else {
                continue;
            };
            let mut first = field;
            while name_at(first - stride).is_some() {
                first -= stride;
            }
            let mut names = Vec::new();
            let mut f = first;
            while let Some(n) = name_at(f) {
                names.push(n);
                f += stride;
            }
            if names.len() > 100 && names.windows(2).all(|w| w[0] < w[1]) {
                tables.push(names);
            }
        }
    }
    tables.dedup();
    match tables.len() {
        1 => Ok(tables.pop().unwrap_or_default()),
        0 => bail!("no sorted icon table holds {ANCHOR}"),
        n => bail!("{n} icon tables hold {ANCHOR}"),
    }
}

fn strings(target: Target) -> Vec<String> {
    target
        .strings()
        .iter()
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Whether a `mov reg, rdx` is among the first 24 instructions at `start`,
/// before anything else writes `rdx`.
fn keeps_rdx(image: &Image<'_>, start: u64) -> bool {
    let Some(code) = image.read(start, 0x80) else {
        return false;
    };
    for insn in Decoder::with_ip(64, code, start, DecoderOptions::NONE)
        .into_iter()
        .take(24)
    {
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Register
            && insn.op1_kind() == OpKind::Register
            && insn.op1_register() == Register::RDX
        {
            return true;
        }
        if insn.op0_kind() == OpKind::Register && insn.op0_register() == Register::RDX {
            return false;
        }
    }
    false
}

/// The callee of the function at `start` called most often whose stride is
/// `item_size`: its `Vec<MenuItem>::push`.
fn most_called_push(image: &Image<'_>, start: u64, item_size: usize) -> Result<u64> {
    let (start, end) = image.function_named("a menu builder", start)?;
    let mut counts: std::collections::BTreeMap<u64, usize> = std::collections::BTreeMap::new();
    for target in calls_after(image, start, usize::try_from(end - start)?) {
        *counts.entry(target).or_default() += 1;
    }
    let mut by_count: Vec<(u64, usize)> = counts.into_iter().collect();
    by_count.sort_by_key(|&(t, n)| (std::cmp::Reverse(n), t));
    by_count
        .into_iter()
        .map(|(t, _)| t)
        .find(|&t| {
            image
                .function(t)
                .is_some_and(|(s, e)| e - s < 0x80 && s == t)
                && push_stride(image, t).is_ok_and(|s| s == item_size)
        })
        .with_context(|| format!("no push of {item_size:#x}-byte items in {start:#x}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the client on disk, when it is there.
    #[test]
    fn resolves_a_build_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let m = Menu::resolve(&image).unwrap();
        assert_eq!(m.build_fn - image.base(), 0x2816d4c);
        assert_eq!((m.item_size, m.props, m.action_tag), (0x1f8, 0x20, 2));
        assert_eq!((m.on_click_tag, m.on_click), (0x8, 0x10));
        assert_eq!((m.message_id, m.channel_id), (0x88, 0x80));
        let rva = |t| {
            let b = Builder::resolve(&image, t, m.item_size).unwrap();
            (b.build_fn - image.base(), b.vec_push - image.base())
        };
        assert_eq!(rva(Target::User), (0x4a16cc9, 0x49c2fc1));
        assert_eq!(rva(Target::Channel), (0x50c0910, 0x5021990));
        assert_eq!(rva(Target::Guild), (0x458fe71, 0x4571269));
        assert_eq!((m.danger, m.id_icon, m.trash_icon), (2, 0x15b, 0x280));
        let icons = icons(&image, &m).unwrap();
        assert_eq!(icons.len(), CONTRACT_ICONS.len());
        // The client's own Reply, Copy Text and Delete Message rows.
        assert_eq!(
            (icons["reply"], icons["copy"], icons["trash"]),
            (0x16, 0x99, 0x280)
        );
    }
}
