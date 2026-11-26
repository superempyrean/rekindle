//! The client's REST request function, found through its typing indicator.
//!
//! The client tells Discord the user is typing with `POST
//! /channels/{id}/typing`. Each place that sends it formats the path from a
//! template holding `/channels/` and `/typing`, then calls the request
//! function shared by every REST call:
//!
//! ```text
//! lea  rdx, [template]
//! call format              ; the path, a String
//! ...
//! lea  rdx, [method]
//! call request             ; (client, &method, path ptr, path len, a, b)
//! ```
//!
//! `request` returns the request's boxed future in `rax:rdx`, and only
//! that future, once polled, reaches the network. Its entry copies the path's
//! pointer and length (`r8`, `r9`) into the future's state.
//!
//! [`resolve`] takes the first call after each template's `format` that
//! every site makes and that stores `r8` and `r9`.

use anyhow::{Context, Result, bail};
use iced_x86::{Code, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::Image;

const CHANNELS: &[u8] = b"/channels/";
const TYPING: &[u8] = b"/typing";
/// How far past `/channels/` the template's `/typing` may start: the bytes
/// between hold the channel id's placeholder.
const PLACEHOLDER_MAX: usize = 6;
/// How many bytes after the template's `lea` the request call is looked for.
const SITE_BYTES: u64 = 0x300;
/// How many bytes of `request`'s entry hold the stores of `r8` and `r9`.
const ENTRY_BYTES: usize = 0x60;

/// The request function and the typing sites that call it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Typing {
    /// The shared request function's entry.
    pub request: u64,
    /// Each template `lea` that leads to it.
    pub sites: Vec<u64>,
}

/// True if `path` is `…/channels/{id}/typing`, with or without a query.
#[must_use]
pub fn is_typing_path(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut seg = path.trim_end_matches('/').rsplit('/');
    seg.next() == Some("typing")
        && seg
            .next()
            .is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
        && seg.next() == Some("channels")
}

/// Where `/channels/` starts in `data` with `/typing` just after it.
fn templates(data: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = data
        .get(from..)
        .and_then(|d| d.windows(CHANNELS.len()).position(|w| w == CHANNELS))
    {
        let at = from + p;
        let after = at + CHANNELS.len();
        let window = data
            .get(after..(after + PLACEHOLDER_MAX + TYPING.len()).min(data.len()))
            .unwrap_or_default();
        if window.windows(TYPING.len()).any(|w| w == TYPING) {
            out.push(at);
        }
        from = after;
    }
    out
}

fn decode(code: &[u8], ip: u64) -> Vec<Instruction> {
    Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
        .into_iter()
        .take_while(|i| !i.is_invalid())
        .collect()
}

/// The direct calls in `code`, in order, after the first one (the path's
/// `format`).
fn calls_after_format(code: &[Instruction]) -> Vec<u64> {
    code.iter()
        .filter(|i| i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch64)
        .map(Instruction::near_branch_target)
        .skip(1)
        .collect()
}

/// True if `entry` stores both `r8` and `r9` to memory.
fn stores_path(entry: &[Instruction]) -> bool {
    let stores = |r: Register| {
        entry.iter().any(|i| {
            i.code() == Code::Mov_rm64_r64
                && i.op0_kind() == OpKind::Memory
                && i.op1_register() == r
        })
    };
    stores(Register::R8) && stores(Register::R9)
}

/// The first call every site makes after its `format` that `is_request`
/// accepts.
fn shared_call(sites: &[Vec<u64>], is_request: impl Fn(u64) -> bool) -> Option<u64> {
    let (first, rest) = sites.split_first()?;
    first
        .iter()
        .copied()
        .find(|t| rest.iter().all(|s| s.contains(t)) && is_request(*t))
}

/// Find the request function in `image`.
pub fn resolve(image: &Image<'_>) -> Result<Typing> {
    let mut leas = Vec::new();
    for (start, end) in image.data() {
        let Some(bytes) = image.read(start, usize::try_from(end - start)?) else {
            continue;
        };
        for at in templates(bytes) {
            let va = start + at as u64;
            // The template's lea may point at a length byte before the text.
            leas.extend(image.leas_to(va - 8, va + 1));
        }
    }
    leas.sort_unstable();
    leas.dedup();
    if leas.is_empty() {
        bail!("no code loads a `/channels/…/typing` template");
    }

    let mut sites = Vec::new();
    for &lea in &leas {
        let (_, end) = image.function_named("typing site", lea)?;
        let len = usize::try_from((end - lea).min(SITE_BYTES))?;
        let code = image.read(lea, len).context("typing site is unmapped")?;
        sites.push(calls_after_format(&decode(code, lea)));
    }
    let request = shared_call(&sites, |t| {
        image
            .read(t, ENTRY_BYTES)
            .is_some_and(|b| stores_path(&decode(b, t)))
    })
    .with_context(|| format!("no request call shared by the typing sites at {leas:#x?}"))?;
    Ok(Typing {
        request,
        sites: leas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn typing_paths() {
        assert!(is_typing_path("/channels/123456789012345678/typing"));
        assert!(is_typing_path(
            "https://discord.com/api/v9/channels/1/typing?x=1"
        ));
        assert!(!is_typing_path("/channels/1/messages"));
        assert!(!is_typing_path("/channels/abc/typing"));
        assert!(!is_typing_path("/users/1/typing"));
        assert!(!is_typing_path("/channels//typing"));
    }

    #[test]
    fn finds_a_template_with_a_placeholder() {
        let data = b"xx\n/channels/\xc0\x07/typing\0\0/channels/1/messages\0/channels/";
        assert_eq!(templates(data), vec![3]);
    }

    /// The two typing sites of a desktop build, from the template's `lea`
    /// to the request call, and the request function's entry.
    const SITE_A: (u64, &str) = (
        0x6d2a067,
        "488d152a502a0f488d8d880100004c8d8578010000e80fb8340190488b8598010000488b8d68020000
         488941580f1085880100000f1141484c8b41504c8b4958488d416048c781200100000000000048c781
         280100000800000048c781300100000000000048c7417002000000c681760200000048c78138010000
         ffffffff0f57c00f11814802000048c78180010000ffffffff48c78198010000ffffffff48c781b001
         0000ffffffff48c781e8010000ffffffffc781680200000000000048c741600000000048c781000200
         00ffffffff48c7816c02000000000000c6817402000000c78120020000ffffffffc78130020000ffff
         ffffc78140020000ffffffff48c7815802000000000000c68175020000004c89b55002000048899d48
         020000488944242848c744242000000000488d15cac8290f4889f9e81812be00",
    );
    const SITE_B: (u64, &str) = (
        0xd5e1,
        "488d15f0be2b13488d8d10140000e88d57010090488b8520140000488987b8000000f30f6f85101400
         00f30f7f87a80000004c8b87b00000004c8b8fb80000004883a7800100000048c78788010000080000
         004883a7900100000048c787d00000000200000031c08887d60200008887c802000048838f98010000
         ff660fefc0f30f7f87a802000048838fe0010000ff48838ff8010000ff48838f10020000ff48838f48
         020000ff6683a7c9020000008887cb0200004883a7c00000000048838f60020000ff4883a7cc020000
         006683a7d402000000838f80020000ff838f90020000ff838fa0020000ff488d87c00000004883a7b8
         02000000488b8dc82e000048898dc8260000488b8dc02e000048898dc0260000488b8db82e00004889
         8db8260000488b8db02e000048898db02600004889442428488364242000488d1563b92b134889f1e8
         91dc8f07",
    );
    const REQUEST: (u64, &str) = (
        0x790b3be,
        "b888190000e8d8e7dd0a4829c44c8b9424b81900004c8b9c24b0190000488d442420488948180f1002
         0f2900488b4a10488948104c8940204c8948284c8958384c895030c68060190000004889c1e8ad0650
         08904881c488190000c3",
    );

    #[test]
    fn the_sites_share_the_request_call() {
        let site = |(ip, h): (u64, &str)| calls_after_format(&decode(&hex(h), ip));
        let (a, b) = (site(SITE_A), site(SITE_B));
        // Each site formats its path with a function of its own.
        assert_ne!(a.first(), None);
        let entry = hex(REQUEST.1);
        let request = shared_call(&[a, b], |t| {
            t == REQUEST.0 && stores_path(&decode(&entry, REQUEST.0))
        });
        assert_eq!(request, Some(REQUEST.0));
        // The format call takes the template, not the path, and stores neither.
        let format = decode(&hex(SITE_A.1), SITE_A.0)
            .iter()
            .find(|i| i.mnemonic() == Mnemonic::Call)
            .map(Instruction::near_branch_target);
        assert_ne!(format, Some(REQUEST.0));
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
        let t = resolve(&image).unwrap();
        eprintln!("{t:#x?}");
        assert!(t.sites.len() >= 2);
        // The entry is movable by an inline detour: `mov eax, imm32`.
        assert_eq!(image.read(t.request, 1), Some(&[0xb8][..]));

        // The same, read from the image laid out as the loader maps it.
        let pe = rekindle_sigmatch::short::PeImage::parse(&bytes).unwrap();
        let size = pe
            .sections
            .iter()
            .map(|s| (s.rva + s.virtual_size.max(s.raw_size)) as usize)
            .max()
            .unwrap();
        let mut mapped = vec![0u8; size];
        mapped[..0x1000].copy_from_slice(&bytes[..0x1000]);
        for s in &pe.sections {
            let n = s.raw_size.min(s.virtual_size.max(s.raw_size)) as usize;
            let from = s.raw_offset as usize;
            let n = n.min(bytes.len() - from);
            mapped[s.rva as usize..s.rva as usize + n].copy_from_slice(&bytes[from..from + n]);
        }
        let base = 0x7ff6_0000_0000;
        let live = resolve(&Image::mapped(&mapped, base).unwrap()).unwrap();
        assert_eq!(live.request - base, t.request - image.base());
        assert_eq!(live.sites.len(), t.sites.len());
    }
}
