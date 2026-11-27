//! The client's own settings components: `mana`'s `Switch` and `Button`,
//! and the settings kit's row and text block, which rekindle builds its
//! settings rows with instead of drawing lookalikes.
//!
//! Each builder is found by the order the client calls them in, around two
//! strings only one settings row loads, and each layout from the code that
//! fills it:
//!
//! - The Developer Mode row loads `set-devmodeDeveloper Mode`, its switch's
//!   id and label in one literal, and right after it calls
//!   `Switch::new(out, id, id_len, checked)`. Of the calls that follow, the
//!   first two that drop and replace a `SharedString` (`movzx eax, byte
//!   [r + at]; cmp al, 0xff; ...; cmp al, 0x19`) are the label's and the
//!   description's setters, and their `at` the fields. The next that stores
//!   an `Rc` and its vtable (`mov [r + at], rax; lea rax, [rip + vtable];
//!   mov [r + at + 8], rax`) is `on_change`, which gives the handler's field
//!   and, from its return copy, the switch's size. The call after it turns
//!   the switch into the row's `Div`.
//! - The tester-updates row loads `tester-updates-sign-in`, its button's
//!   id, and calls `Button::new(out, id, id_len, &label)`. The next call
//!   that stores an `Rc` is `on_click`, which gives the handler's field and
//!   the button's size. The row is the next call whose first call is
//!   `div()`: `ActionRow(out, &title, &subtitle, &mut button)`. Inside it the
//!   text block, `TextBlock::new(out, &title, &subtitle)`, is the call that
//!   takes copies of both texts, `Div::child::<Div>` the call after `div()`
//!   the second time, and `Div::child::<Button>` the call that takes the
//!   button.
//! - A button's variant and size are bytes the client's callers set right
//!   after `Button::new`. Over all of them, the field set most often is the
//!   variant and the next the size.
//!
//! `mana`'s heading and divider are found apart from these, by their type
//! names ([`kit`]).

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::{Image, one_of};

const DEVELOPER_MODE: &[u8] = b"set-devmodeDeveloper Mode";
const SIGN_IN: &[u8] = b"tester-updates-sign-in";
/// How far after each string its row's calls are read.
const SWITCH_SPAN: u64 = 0x300;
const BUTTON_SPAN: u64 = 0x200;

/// `mana`'s switch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Switch {
    /// `Switch::new(out, id: *const u8, id_len, checked: bool) -> out`. The
    /// id is a `&'static str`.
    pub new: u64,
    /// `(out: *mut Div, switch: *mut Switch) -> out`, the settings row.
    pub into_div: u64,
    pub size: usize,
    /// `SharedString`s.
    pub label: usize,
    pub description: usize,
    /// `Option<Rc<dyn Fn(bool, &mut Window, &mut App)>>`.
    pub on_change: usize,
}

/// `mana`'s button.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Button {
    /// `Button::new(out, id: *const u8, id_len, label: *const SharedString)
    /// -> out`, a leaf function without an unwind entry.
    pub new: u64,
    pub size: usize,
    /// `Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>`.
    pub on_click: usize,
    /// A byte: 0 is the primary look.
    pub variant: usize,
    /// A byte: 2 is the medium size.
    pub scale: usize,
}

/// The settings components and the `Div` calls they're put together with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Components {
    pub switch: Switch,
    pub button: Button,
    /// `ActionRow(out: *mut Div, title, subtitle: *const SharedString,
    /// button: *mut Button) -> out`.
    pub action_row: u64,
    /// `TextBlock::new(out: *mut Div, title, subtitle: *const SharedString)
    /// -> out`.
    pub text_block: u64,
    /// `Div::child::<Div>(out, parent: *mut Div, child: *mut Div) -> out`.
    pub child_div: u64,
    /// `Div::child::<Button>(out, parent: *mut Div, button: *mut Button) ->
    /// out`.
    pub child_button: u64,
}

fn decode(code: &[u8], ip: u64) -> Vec<Instruction> {
    Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
        .into_iter()
        .take_while(|i| !i.is_invalid())
        .collect()
}

/// The instructions from `va`, `len` bytes of them.
fn code_at(image: &Image<'_>, va: u64, len: u64) -> Result<Vec<Instruction>> {
    let code = image
        .read(va, usize::try_from(len)?)
        .with_context(|| format!("{va:#x} is unmapped"))?;
    Ok(decode(code, va))
}

/// The function at `va`, to its unwind entry's end, or `len` bytes of it if
/// it has none.
fn function_code(image: &Image<'_>, va: u64, len: u64) -> Result<Vec<Instruction>> {
    match image.function(va) {
        Some((start, end)) if start == va => code_at(image, va, end - va),
        _ => code_at(image, va, len),
    }
}

/// The direct calls in `insns`, as `(site, target)`.
fn calls(insns: &[Instruction]) -> Vec<(u64, u64)> {
    insns
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch64)
        .map(|i| (i.ip(), i.near_branch_target()))
        .collect()
}

/// The one `lea r, [rip + string]` of the string `text`, which must occur
/// once.
fn only_load(image: &Image<'_>, text: &[u8]) -> Result<u64> {
    let name = String::from_utf8_lossy(text);
    let at = one_of(&format!("the string {name:?}"), &image.find(text))?;
    let leas: Vec<u64> = image.leas_into(&[at]).iter().map(|l| l.0).collect();
    one_of(&format!("the load of {name:?}"), &leas)
}

/// The field a `SharedString` setter replaces: the first `movzx r32, byte
/// [r + at]` followed by `cmp r8, 0xff` and, within a few instructions,
/// `cmp r8, 0x19` (a `SharedString` of an `Arc<str>`, whose count the
/// setter drops).
fn shared_string_field(insns: &[Instruction]) -> Option<usize> {
    insns.windows(6).find_map(|w| {
        let load = &w[0];
        let cmp = |i: &Instruction, value: u64| {
            i.mnemonic() == Mnemonic::Cmp
                && i.op0_kind() == OpKind::Register
                && i.op0_register() == Register::AL
                && i.op1_kind() == OpKind::Immediate8
                && u64::from(i.immediate8()) == value
        };
        (load.mnemonic() == Mnemonic::Movzx
            && load.op0_register() == Register::EAX
            && load.op1_kind() == OpKind::Memory
            && load.memory_index() == Register::None
            && cmp(&w[1], 0xff)
            && w[2..].iter().any(|i| cmp(i, 0x19)))
        .then(|| usize::try_from(load.memory_displacement64()).ok())
        .flatten()
    })
}

/// Where a builder stores an `Rc` and its vtable:
/// `mov [r + at], rax; lea rax, [rip + vtable]; mov [r + at + 8], rax`.
pub(crate) fn rc_field(insns: &[Instruction]) -> Option<usize> {
    insns.windows(3).find_map(|w| {
        let store = |i: &Instruction| {
            (i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Memory
                && i.memory_index() == Register::None
                && i.op1_kind() == OpKind::Register
                && i.op1_register() == Register::RAX)
                .then(|| (i.memory_base(), i.memory_displacement64()))
        };
        let (Some((base, at)), lea, Some((base2, at2))) = (store(&w[0]), &w[1], store(&w[2]))
        else {
            return None;
        };
        (lea.mnemonic() == Mnemonic::Lea
            && lea.memory_base() == Register::RIP
            && lea.op0_register() == Register::RAX
            && base == base2
            && at2 == at + 8)
            .then(|| usize::try_from(at).ok())
            .flatten()
    })
}

/// The size a builder returns its value with: the largest `mov ecx, n`
/// before a `rep movsb`, or `mov r8d, n` before a call (`memcpy`).
fn return_size(insns: &[Instruction]) -> Option<usize> {
    insns
        .windows(4)
        .filter_map(|w| {
            let n = |reg| {
                (w[0].mnemonic() == Mnemonic::Mov
                    && w[0].op0_kind() == OpKind::Register
                    && w[0].op0_register() == reg
                    && w[0].op1_kind() == OpKind::Immediate32)
                    .then(|| w[0].immediate32() as usize)
            };
            let copies = w[1..]
                .iter()
                .any(|i| i.has_rep_prefix() && i.mnemonic() == Mnemonic::Movsb);
            let calls = w[1..].iter().any(|i| i.mnemonic() == Mnemonic::Call);
            match (n(Register::ECX), n(Register::R8D)) {
                (Some(n), _) if copies => Some(n),
                (_, Some(n)) if calls => Some(n),
                _ => None,
            }
        })
        .max()
}

fn switch(image: &Image<'_>) -> Result<Switch> {
    let lea = only_load(image, DEVELOPER_MODE)?;
    let row = calls(&code_at(image, lea, SWITCH_SPAN)?);
    let &(_, new) = row
        .first()
        .context("no call after the Developer Mode row's id")?;
    // A setter returns the switch's leading part, a copy of the same size
    // each time. The texts the row makes in between return other things.
    let mut setters: Vec<(usize, Option<usize>)> = Vec::new();
    let mut on_change = None;
    for (k, &(_, target)) in row.iter().enumerate().skip(1) {
        if image.function(target).map(|f| f.0) != Some(target) {
            continue;
        }
        let body = function_code(image, target, 0x400)?;
        if let Some(at) = rc_field(&body) {
            on_change = Some((k, target, at, return_size(&body)));
            break;
        }
        if let Some(at) = shared_string_field(&body) {
            setters.push((at, return_size(&body)));
        }
    }
    let (k, _, on_change, size) = on_change.context("no Switch::on_change after Switch::new")?;
    let size = size.context("Switch::on_change returns no copy")?;
    let mut by_size: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (at, copied) in setters {
        if let Some(n) = copied.filter(|&n| n < size) {
            by_size.entry(n).or_default().push(at);
        }
    }
    let Some([label, description]) = by_size
        .values()
        .find_map(|fields| <[usize; 2]>::try_from(fields.as_slice()).ok())
    else {
        bail!("no two SharedString setters of one part before on_change: {by_size:x?}");
    };
    let &(_, into_div) = row.get(k + 1).context("no call after Switch::on_change")?;
    let copies = return_size(&function_code(image, into_div, 0x400)?);
    if copies != Some(size) {
        bail!("the call after on_change copies {copies:?} bytes, not a Switch of {size:#x}");
    }
    if [label + 24, description + 24, on_change + 16]
        .iter()
        .any(|&end| end > size)
    {
        bail!("a Switch field past its {size:#x} bytes");
    }
    Ok(Switch {
        new,
        into_div,
        size,
        label,
        description,
        on_change,
    })
}

/// The byte fields the client's callers set most often right after
/// `Button::new`, ranked: `(times, offset)`.
fn byte_stores_after(image: &Image<'_>, new: u64, size: usize) -> Vec<(usize, usize)> {
    let mut count: BTreeMap<usize, usize> = BTreeMap::new();
    for (site, _) in image.calls_into(&[new]) {
        let Ok(after) = code_at(image, site + 5, 0x60) else {
            continue;
        };
        // `lea r, [rbp + out]` then `mov byte [r + at], ...`, before the
        // button is copied or passed on.
        let mut base = None;
        for i in after.iter().take(8) {
            if i.mnemonic() == Mnemonic::Call || i.has_rep_prefix() {
                break;
            }
            if i.mnemonic() == Mnemonic::Lea && i.memory_base() != Register::RIP {
                base = Some(i.op0_register());
            }
            if i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Memory
                && i.memory_size().size() == 1
                && Some(i.memory_base()) == base
                && let Ok(at) = usize::try_from(i.memory_displacement64())
                && at < size
            {
                *count.entry(at).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<(usize, usize)> = count.into_iter().map(|(at, n)| (n, at)).collect();
    ranked.sort_unstable_by(|a, b| b.cmp(a));
    ranked
}

fn button(image: &Image<'_>, div_new: u64) -> Result<(Button, u64, u64, u64, u64)> {
    let lea = only_load(image, SIGN_IN)?;
    let row = calls(&code_at(image, lea, BUTTON_SPAN)?);
    let &(_, new) = row
        .first()
        .context("no call after the sign-in button's id")?;
    let mut on_click = None;
    for (k, &(_, target)) in row.iter().enumerate().skip(1) {
        let body = function_code(image, target, 0x400)?;
        if let Some(at) = rc_field(&body) {
            on_click = Some((k, at, return_size(&body)));
            break;
        }
    }
    let (k, on_click, size) = on_click.context("no Button::on_click after Button::new")?;
    let size = size.context("Button::on_click returns no copy")?;
    let mut action_row = None;
    for &(_, target) in &row[k + 1..] {
        let body = function_code(image, target, 0x800)?;
        let inner = calls(&body);
        if inner.first().map(|c| c.1) == Some(div_new) {
            action_row = Some((target, inner));
            break;
        }
    }
    let (action_row, inner) = action_row.context("no ActionRow after Button::on_click")?;
    let divs: Vec<usize> = inner
        .iter()
        .enumerate()
        .filter(|(_, c)| c.1 == div_new)
        .map(|(k, _)| k)
        .collect();
    let [_, second, ..] = divs[..] else {
        bail!("ActionRow builds {} Divs, want 2", divs.len());
    };
    let child_div = inner[second - 1].1;
    let body = function_code(image, action_row, 0x800)?;
    let text_block = text_block(&body, &inner[..second])?;
    // `Div::child::<Button>` is the call just before the last
    // `Div::child::<Div>`, which puts the button's Div in the row.
    let last = inner
        .iter()
        .rposition(|c| c.1 == child_div)
        .context("ActionRow adds no child")?;
    let child_button = inner
        .get(last.wrapping_sub(1))
        .filter(|_| last > second)
        .context("no Div::child::<Button> in ActionRow")?
        .1;
    let ranked = byte_stores_after(image, new, size);
    let (variant, scale) = match ranked[..] {
        [(first, variant), (second, scale), ..] if first >= 3 * second => (variant, scale),
        _ => bail!(
            "the bytes set after Button::new don't single out the variant: {:x?}",
            &ranked[..ranked.len().min(4)]
        ),
    };
    if on_click + 16 > size || variant >= size || scale >= size {
        bail!("a Button field past its {size:#x} bytes");
    }
    Ok((
        Button {
            new,
            size,
            on_click,
            variant,
            scale,
        },
        action_row,
        text_block,
        child_div,
        child_button,
    ))
}

/// `TextBlock::new`: of `calls`, the action row's before its second
/// `div()`, the one the row passes its two texts to right after copying
/// them. `body` is the action row.
fn text_block(body: &[Instruction], calls: &[(u64, u64)]) -> Result<u64> {
    let mut found = Vec::new();
    for &(site, target) in calls {
        let Some(at) = body.iter().position(|i| i.ip() == site) else {
            continue;
        };
        let before = &body[at.saturating_sub(16)..at];
        // The copies come out of the arguments ActionRow keeps: two
        // `movups xmm0, [rcx]` loads, of the title and the subtitle.
        let loads = before
            .iter()
            .filter(|i| {
                i.mnemonic() == Mnemonic::Movups
                    && i.op1_kind() == OpKind::Memory
                    && i.memory_base() == Register::RCX
                    && i.memory_displacement64() == 0
            })
            .count();
        let r8_lea = before
            .iter()
            .any(|i| i.mnemonic() == Mnemonic::Lea && i.op0_register() == Register::R8);
        if loads == 2 && r8_lea {
            found.push(target);
        }
    }
    // The calls after it see the same copies before them, so the first
    // is the text block.
    found
        .first()
        .copied()
        .context("TextBlock::new: no call after the copies of ActionRow's texts")
}

/// Find the components in `image`, whose `div()` is `div_new`.
pub fn resolve(image: &Image<'_>, div_new: u64) -> Result<Components> {
    let switch = switch(image).context("Switch")?;
    let (button, action_row, text_block, child_div, child_button) =
        button(image, div_new).context("Button")?;
    Ok(Components {
        switch,
        button,
        action_row,
        text_block,
        child_div,
        child_button,
    })
}

/// A `mana` component that is built with a constructor and put in a `Div`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Part {
    /// The constructor, `new(out, ...) -> out`.
    pub new: u64,
    /// `Div::child::<Part>(out, parent: *mut Div, part: *mut Part) -> out`.
    pub child: u64,
    pub size: usize,
}

/// `mana`'s heading and divider.
///
/// - `Heading::new(out, size: u16, text: *const SharedString) -> out`, a
///   leaf function.
/// - `Divider::new(out) -> out`, a horizontal divider in the theme's colour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Kit {
    pub heading: Part,
    pub divider: Part,
}

const HEADING: &[u8] = b"mana_gpui::components::heading::Heading";
const DIVIDER: &[u8] = b"mana_gpui::components::divider::Divider";

/// Find the heading and the divider in `image`, whose `Div` is `div_size`
/// bytes.
///
/// Each component's `into_any_element` is the small function that loads
/// its type name and copies the component (`mov r8d, size; call memcpy`).
/// `Div::child::<T>` is the small function whose first call is that one
/// and which returns a `Div`. The constructor is the call the client makes
/// most often right before `Div::child::<T>`.
pub fn kit(image: &Image<'_>, div_size: usize) -> Result<Kit> {
    Ok(Kit {
        heading: part(image, HEADING, div_size).context("Heading")?,
        divider: part(image, DIVIDER, div_size).context("Divider")?,
    })
}

fn part(image: &Image<'_>, type_name: &[u8], div_size: usize) -> Result<Part> {
    let (into_any, size) = into_any_of(image, type_name)?;
    // `Div::child::<T>`: callers of it that return a `Div`.
    let mut children: BTreeMap<u64, usize> = BTreeMap::new();
    for (site, _) in image.calls_into(&[into_any]) {
        let Some((start, end)) = image.function(site) else {
            continue;
        };
        if end - start > 0x90 {
            continue;
        }
        let body = code_at(image, start, end - start)?;
        if calls(&body).first().map(|c| c.1) == Some(into_any)
            && return_size(&body) == Some(div_size)
        {
            children.insert(start, 0);
        }
    }
    let targets: Vec<u64> = children.keys().copied().collect();
    // The call before each call of a `Div::child::<T>`.
    let mut before: BTreeMap<u64, usize> = BTreeMap::new();
    for (site, child) in image.calls_into(&targets) {
        *children.entry(child).or_default() += 1;
        let Some((start, _)) = image.function(site) else {
            continue;
        };
        let body = code_at(image, start, site - start)?;
        if let Some(&(_, new)) = calls(&body).last() {
            *before.entry(new).or_default() += 1;
        }
    }
    let child = children
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(c, _)| *c)
        .context("no Div::child for the component")?;
    let (new, uses) = before
        .iter()
        .max_by_key(|(_, n)| **n)
        .map(|(c, n)| (*c, *n))
        .context("no constructor before Div::child")?;
    if uses < 3 {
        bail!("the constructor {new:#x} comes before Div::child only {uses} times");
    }
    Ok(Part { new, child, size })
}

/// `mana`'s modal: a dialog over the client's window, with its own card,
/// scrim and close.
///
/// - `Modal::new(out, title: *const SharedString) -> out` fills a modal
///   with its title and no content. The client also has a constructor
///   without a title, `(out, 1, 0)`, copied into each codegen unit that
///   uses it, which it calls far less.
/// - `body` and `footer` are `Vec<AnyElement>`s: the client's builders push
///   a child's element into `body` and a button's into `footer` with
///   `push(vec, element)`.
/// - `on_close` is an `Option<Rc<dyn Fn(&mut Window, &mut App)>>`, `None`
///   in a fresh modal.
/// - `into_any(out, modal: *mut Modal) -> out` moves it into an
///   `AnyElement`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Modal {
    pub new: u64,
    pub into_any: u64,
    pub size: usize,
    pub body: usize,
    pub footer: usize,
    pub on_close: usize,
    /// `Vec<AnyElement>::push(vec: *mut Vec, element: *const AnyElement)`.
    pub push: u64,
}

const MODAL: &[u8] = b"mana_html::hosted::modal::Modal";
/// The element id each copy of `Modal::new` loads.
const MODAL_ID: &[u8] = b"mana-modal";
/// The largest function taken for one of the modal's builders.
const SETTER_SPAN: u64 = 0x100;

/// `into_any_element` of the component whose type name is `type_name`: the
/// small function that loads the name and copies the component, with the
/// component's size.
fn into_any_of(image: &Image<'_>, type_name: &[u8]) -> Result<(u64, usize)> {
    let mut converts = Vec::new();
    for (lea, _) in image.leas_into(&image.find(type_name)) {
        let Some((start, end)) = image.function(lea) else {
            continue;
        };
        if end - start > 0x100 {
            continue;
        }
        if let Some(size) = return_size(&code_at(image, start, end - start)?) {
            converts.push((start, size));
        }
    }
    converts.sort_unstable();
    converts.dedup();
    one_of("the component's into_any_element", &converts)
}

/// What one of the modal's builders does with the modal it takes:
/// `lea rcx, [r + at]`, the element into `rdx` and `call push`.
fn pushes(body: &[Instruction]) -> Option<(usize, u64)> {
    body.iter().enumerate().find_map(|(k, i)| {
        if i.mnemonic() != Mnemonic::Lea
            || i.op0_register() != Register::RCX
            || i.memory_base() == Register::RIP
            || i.memory_base() == Register::RSP
            || i.memory_base() == Register::RBP
            || i.memory_index() != Register::None
        {
            return None;
        }
        let next = &body[k + 1..body.len().min(k + 4)];
        let at = next
            .iter()
            .position(|c| c.mnemonic() == Mnemonic::Call && c.op0_kind() == OpKind::NearBranch64)?;
        // A push takes the element in `rdx`; a drop of the field doesn't.
        if !next[..at].iter().any(|c| {
            matches!(c.mnemonic(), Mnemonic::Lea | Mnemonic::Mov)
                && c.op0_kind() == OpKind::Register
                && c.op0_register() == Register::RDX
        }) {
            return None;
        }
        let call = &next[at];
        Some((
            usize::try_from(i.memory_displacement64()).ok()?,
            call.near_branch_target(),
        ))
    })
}

/// The most frequent key of `counts`, if it is ahead of the next.
fn most<K: Copy + Ord + std::fmt::Debug>(what: &str, counts: &BTreeMap<K, usize>) -> Result<K> {
    let mut ranked: Vec<(usize, K)> = counts.iter().map(|(k, n)| (*n, *k)).collect();
    ranked.sort_unstable_by(|a, b| b.cmp(a));
    match ranked.first() {
        Some(&(n, k)) if n > ranked.get(1).map_or(0, |r| r.0) => Ok(k),
        _ => bail!("{what}: no clear choice in {ranked:x?}"),
    }
}

/// Find `mana`'s modal in `image`.
///
/// `into_any_element` loads the modal's type name, as the heading's does.
/// The functions that call it build modals. Of the calls each of them
/// makes, `Modal::new` is the first to a function that loads the modal's
/// id, and of those, the one most of them call. The modal's builders
/// are the small functions that return a copy of the modal's size, and the
/// ones those call. One that stores an `Rc` and its vtable gives
/// `on_close`. The others that pass a field of the modal to a call (`lea
/// rcx, [r + at]; call`) push an element into it. The function most of them
/// call is `push`, the field most of them push into `body`, and the next
/// `footer`, which only buttons go into: the client has one builder for
/// each type of child, and few for buttons.
pub fn modal(image: &Image<'_>) -> Result<Modal> {
    let (into_any, size) = into_any_of(image, MODAL).context("Modal::into_any_element")?;
    let id_loads: Vec<u64> = image
        .leas_into(&image.find(MODAL_ID))
        .iter()
        .map(|l| l.0)
        .collect();
    let loads_id = |f: u64| -> bool {
        image
            .function(f)
            .is_some_and(|(start, end)| id_loads.iter().any(|lea| (start..end).contains(lea)))
    };
    let mut news: BTreeMap<u64, usize> = BTreeMap::new();
    let mut setters: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    let mut builders = Vec::new();
    for (site, _) in image.calls_into(&[into_any]) {
        if let Some((start, _)) = image.function(site) {
            builders.push(start);
        }
    }
    builders.sort_unstable();
    builders.dedup();
    let small_copy = |f: u64| -> Option<Vec<Instruction>> {
        let (start, end) = image.function(f)?;
        if start != f || end - start > SETTER_SPAN {
            return None;
        }
        let body = code_at(image, start, end - start).ok()?;
        (return_size(&body) == Some(size)).then_some(body)
    };
    for builder in builders {
        let Some((start, end)) = image.function(builder) else {
            continue;
        };
        let body = code_at(image, start, end - start)?;
        let targets: Vec<u64> = calls(&body).iter().map(|c| c.1).collect();
        if let Some(&new) = targets.iter().find(|&&t| loads_id(t)) {
            *news.entry(new).or_default() += 1;
        }
        for t in targets {
            if t == into_any {
                continue;
            }
            if let Some(body) = small_copy(t) {
                setters.insert(t);
                for (_, inner) in calls(&body) {
                    if small_copy(inner).is_some() {
                        setters.insert(inner);
                    }
                }
            }
        }
    }
    let new = most("Modal::new", &news)?;
    // The builders that store an `Rc` store `on_close`; of the others, the
    // ones that push an element into the modal call one function to push,
    // into `body` for most of them and into `footer` for the next most.
    let mut closes: BTreeMap<usize, usize> = BTreeMap::new();
    let mut pushed: BTreeMap<(u64, usize), usize> = BTreeMap::new();
    for &setter in &setters {
        let Some(body) = small_copy(setter) else {
            continue;
        };
        if let Some(at) = rc_field(&body) {
            *closes.entry(at).or_default() += 1;
        } else if let Some((at, push)) = pushes(&body) {
            *pushed.entry((push, at)).or_default() += 1;
        }
    }
    let mut by_push: BTreeMap<u64, usize> = BTreeMap::new();
    for (&(push, _), &n) in &pushed {
        *by_push.entry(push).or_default() += n;
    }
    let push = most("Vec<AnyElement>::push", &by_push).with_context(|| format!("{pushed:x?}"))?;
    let mut fields: Vec<(usize, usize)> = pushed
        .iter()
        .filter(|((p, _), _)| *p == push)
        .map(|((_, at), n)| (*n, *at))
        .collect();
    fields.sort_unstable_by(|a, b| b.cmp(a));
    let [(nb, body), (nf, footer), ..] = fields[..] else {
        bail!("the modal's builders push into {fields:x?}, want a body and a footer");
    };
    if nb <= nf {
        bail!("the modal's body and footer are pushed alike: {fields:x?}");
    }
    let on_close = most("the modal's on_close", &closes)?;
    if [body, footer, on_close].iter().any(|&f| f + 0x18 > size) {
        bail!("a Modal field past its {size:#x} bytes");
    }
    Ok(Modal {
        new,
        into_any,
        size,
        body,
        footer,
        on_close,
        push,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The modal of the client on disk, when it is there.
    #[test]
    fn finds_the_modal_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let m = modal(&image).unwrap();
        eprintln!("{m:#x?}");
        let rva = |va: u64| va - image.base();
        assert_eq!(rva(m.new), 0x9883b90);
        assert_eq!((rva(m.into_any), m.size), (0x99724a0, 0x358));
        assert_eq!((m.body, m.footer, m.on_close), (0x18, 0x30, 0x208));
        assert_eq!(rva(m.push), 0xc428c);
    }

    /// The heading and the divider of the client on disk, when it is there.
    #[test]
    fn finds_the_kit_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let div = crate::div::resolve(&image).unwrap();
        let k = kit(&image, div.size).unwrap();
        let rva = |va: u64| va - image.base();
        assert_eq!((rva(k.heading.new), k.heading.size), (0x8e9a2fe, 0xc0));
        assert_eq!((rva(k.divider.new), k.divider.size), (0x8c923c0, 0x298));
        assert!([0x8ec91f9, 0x8f054d8].contains(&rva(k.heading.child)));
        assert!([0x8ec916b, 0x8f0544a].contains(&rva(k.divider.child)));
    }

    /// Against the client on disk, when it is there.
    #[test]
    fn resolves_a_build_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let div = crate::div::resolve(&image).unwrap();
        let c = resolve(&image, div.new).unwrap();
        eprintln!("{c:#x?}");
        let rva = |va: u64| va - image.base();
        let s = &c.switch;
        assert_eq!((rva(s.new), rva(s.into_div)), (0x8b8d766, 0x15c1f1));
        assert_eq!(
            (s.size, s.label, s.description, s.on_change),
            (0x248, 0x98, 0xb0, 0x230)
        );
        let b = &c.button;
        assert_eq!(rva(b.new), 0x9139375);
        assert_eq!(
            (b.size, b.on_click, b.variant, b.scale),
            (0x328, 0x298, 0x323, 0x31d)
        );
        assert_eq!(rva(c.action_row), 0x1f4f66);
        assert_eq!(rva(c.text_block), 0x14b98b);
        assert_eq!(rva(c.child_div), 0x842bc60);
        assert_eq!(rva(c.child_button), 0x8cf0198);
    }
}
