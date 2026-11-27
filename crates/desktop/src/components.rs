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
