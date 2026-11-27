//! The client's REST transport and its gateway, found by code shape.
//!
//! Every REST request the client makes goes through one function,
//! `ReqwestTransport::send(&self, request) -> Pin<Box<dyn Future>>`, the
//! only function that loads `a host file body needs the host's transport`.
//! It takes the request by pointer and owns it: the headers (an
//! `http::HeaderMap`, `0x60` bytes) at its start, then the body, the URL and
//! the method. `send` copies the method and the URL `String`s into `r8` and
//! `r9` for the call that starts the `reqwest` request, then moves the
//! headers out whole, so both offsets are read off those copies. The body
//! is the enum `send` dispatches on through a jump table, a tag and then,
//! for tag 1, a `bytes::Bytes` `{vtable, ptr, len, data}`.
//!
//! `send` boxes its future in a small function that allocates it and loads
//! its vtable with `lea rdx`. The vtable's `poll(out, future, cx)` returns
//! a `Poll<Result<Response, Error>>` through `out`. A build whose `poll`
//! copies `0x80` bytes into `out` keeps the response's body in a `Vec<u8>`
//! after its `0x60`-byte headers, and its status, a `u16`, after that.
//! Pending is the tag `u64::MAX - 1` in the first word, and an error
//! `u64::MAX`.
//!
//! The box's allocation goes through the client's `__rust_alloc(size,
//! align)`, and `__rust_dealloc(ptr, size, align)` hands the same allocator
//! static to its implementation, then counts the freed bytes down with
//! `lock sub`. Bodies rekindle replaces are allocated and freed with them.
//!
//! Gateway dispatches go through a table libdiscord builds once at start
//! from static slices of `{name: &str, handler}`, one slice per domain,
//! listed in descriptors `{domain: &str, id, slice: *const, len}` of 40
//! bytes. The table copies each handler pointer, so a handler swapped in
//! the slices after the table is built is never called. Each handler is
//! `handler(out, store, event, ctx) -> out`, with the dispatch's name and
//! its JSON as two `&str`s at the start of `event`. `Ok(())` is the tag
//! `u64::MAX - 1` in `out`'s first word.
//!
//! Frames the client sends go through its socket sink's `start_send(out,
//! sink, message) -> out`, the fifth slot of a `Sink` vtable. The function
//! that loads `gateway connect adopted the early socket` loads that vtable,
//! and its `start_send` alone loads `assertion failed: this.state.is_none()`.
//! A text frame is the message's tag 2 and a `String` after it.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use iced_x86::{Code, Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::{Image, one_of};

const TRANSPORT_ANCHOR: &[u8] = b"a host file body needs the host's transport";
const ADOPT_ANCHOR: &[u8] = b"gateway connect adopted the early socket";
const START_SEND_PANIC: &[u8] = b"assertion failed: this.state.is_none()";
/// The event every build handles, which seeds the registry walk.
const SEED: &[u8] = b"MESSAGE_CREATE";

/// The size of `http::HeaderMap`, at the start of the request and of the
/// response.
pub const HEADER_MAP: usize = 0x60;
/// A `String`'s size, and a `Bytes`'.
const STRING: usize = 0x18;
const BYTES: usize = 0x20;
/// The size of `poll`'s result when the body is a `Vec`.
pub const BUFFERED_RESULT: usize = 0x80;
/// `Poll::Pending`, and `Ok(())` from a gateway handler or a sink.
pub const PENDING: u64 = u64::MAX - 1;
/// The size of a gateway registry entry and of a domain descriptor.
const ENTRY: u64 = 24;
const DESCRIPTOR: u64 = 40;
/// The slot of `start_send` in a `Sink` vtable.
const START_SEND_SLOT: u64 = 32;
/// The text frame's tag in the sink's message.
pub const TEXT_FRAME: u64 = 2;

/// Where the request `send` takes keeps the fields rekindle reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RequestLayout {
    /// The body's tag; a `Bytes` follows it for tag 1.
    pub body: usize,
    /// The URL `String`, `{cap, ptr, len}`.
    pub url: usize,
    /// The `http::Method`.
    pub method: usize,
}

/// Where `poll`'s result keeps the response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ResponseLayout {
    /// The body `Vec<u8>`, `{cap, ptr, len}`.
    pub body: usize,
    /// The `u16` status.
    pub status: usize,
}

/// The REST transport.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Transport {
    /// `ReqwestTransport::send`.
    pub send: u64,
    pub request: RequestLayout,
    /// `SendFuture::poll`, and every vtable slot that holds it.
    pub poll: u64,
    pub poll_slots: Vec<u64>,
    pub response: ResponseLayout,
    /// `__rust_alloc(size, align)` and `__rust_dealloc(ptr, size, align)`.
    pub alloc: u64,
    pub dealloc: u64,
}

/// One gateway event's handler and the slices' slots that hold it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Handler {
    pub name: String,
    pub handler: u64,
    /// The handler fields, one per copy of the slices.
    pub slots: Vec<u64>,
}

/// The gateway: incoming dispatches' handlers and the outgoing sink.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Gateway {
    pub handlers: Vec<Handler>,
    /// The sink's `start_send`, and every vtable slot that holds it.
    pub start_send: u64,
    pub start_send_slots: Vec<u64>,
}

fn decode(code: &[u8], ip: u64) -> Vec<Instruction> {
    Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
        .into_iter()
        .take_while(|i| !i.is_invalid())
        .collect()
}

fn function_code(image: &Image<'_>, what: &str, va: u64) -> Result<(u64, u64, Vec<Instruction>)> {
    let (start, end) = image.function_named(what, va)?;
    let code = image
        .read(start, usize::try_from(end - start)?)
        .with_context(|| format!("{what} at {start:#x} is unmapped"))?;
    Ok((start, end, decode(code, start)))
}

fn is_start(image: &Image<'_>, va: u64) -> bool {
    image.function(va).is_some_and(|(s, _)| s == va)
}

/// The `lea r, [rip + x]` targets in `insns`.
fn rip_leas(insns: &[Instruction]) -> impl Iterator<Item = (Register, u64)> + '_ {
    insns
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Lea && i.memory_base() == Register::RIP)
        .map(|i| (i.op0_register(), i.ip_rel_memory_address()))
}

fn direct_calls(insns: &[Instruction]) -> impl Iterator<Item = (usize, u64)> + '_ {
    insns
        .iter()
        .enumerate()
        .filter(|(_, i)| i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch64)
        .map(|(k, i)| (k, i.near_branch64()))
}

/// The immediate operand `n` of `i`, sign-extended, if it is one.
fn imm(i: &Instruction, n: u32) -> Option<u64> {
    matches!(
        i.op_kind(n),
        OpKind::Immediate8
            | OpKind::Immediate16
            | OpKind::Immediate32
            | OpKind::Immediate64
            | OpKind::Immediate8to16
            | OpKind::Immediate8to32
            | OpKind::Immediate8to64
            | OpKind::Immediate32to64
    )
    .then(|| i.immediate(n))
}

/// A memory operand's displacement as an offset into its base.
fn disp(i: &Instruction) -> Option<usize> {
    usize::try_from(i.memory_displacement64() as i64).ok()
}

/// The fields `send` copies into `r8` and `r9` before a call: each
/// `movups xmm, [base + d]` stored with `movaps [r8|r9], xmm`.
fn argument_copies(insns: &[Instruction]) -> Vec<(usize, Register, usize)> {
    let mut out = Vec::new();
    for (k, pair) in insns.windows(2).enumerate() {
        let [load, store] = pair else { continue };
        if load.mnemonic() == Mnemonic::Movups
            && load.op1_kind() == OpKind::Memory
            && load.memory_base() != Register::RIP
            && load.memory_base() != Register::RBP
            && load.memory_base() != Register::RSP
            && matches!(store.mnemonic(), Mnemonic::Movaps | Mnemonic::Movups)
            && store.op0_kind() == OpKind::Memory
            && store.memory_displacement64() == 0
            && store.op1_register() == load.op0_register()
            && matches!(store.memory_base(), Register::R8 | Register::R9)
            && let Some(d) = disp(load)
        {
            out.push((k, store.memory_base(), d));
        }
    }
    out
}

/// The method's and the URL's offsets: the last copies into `r8` and `r9`
/// before the call that the headers' `0x60`-byte move follows.
fn method_and_url(insns: &[Instruction]) -> Result<(usize, usize)> {
    let moves = insns.windows(2).enumerate().filter_map(|(k, w)| {
        (w[0].code() == Code::Pushq_imm8
            && imm(&w[0], 0) == Some(HEADER_MAP as u64)
            && w[1].mnemonic() == Mnemonic::Pop
            && w[1].op0_register() == Register::RCX)
            .then_some(k)
    });
    let copies = argument_copies(insns);
    for at in moves {
        let Some((call, _)) = direct_calls(&insns[..at]).last() else {
            continue;
        };
        let before = |r: Register| {
            copies
                .iter()
                .filter(|(k, reg, _)| *k < call && *reg == r)
                .map(|&(_, _, d)| d)
                .next_back()
        };
        if let (Some(m), Some(u)) = (before(Register::R8), before(Register::R9)) {
            return Ok((m, u));
        }
    }
    bail!("send copies no method and URL into a call before it moves the headers")
}

/// The body's tag: `lea r, [base + d]` whose `[r]` indexes a jump table.
fn body_tag(insns: &[Instruction]) -> Result<usize> {
    let mut found = BTreeSet::new();
    for (k, lea) in insns.iter().enumerate() {
        if lea.mnemonic() != Mnemonic::Lea
            || lea.memory_base() == Register::RIP
            || lea.memory_base() == Register::RBP
            || lea.memory_base() == Register::RSP
            || lea.memory_index() != Register::None
        {
            continue;
        }
        let r = lea.op0_register();
        let tail = &insns[k + 1..(k + 24).min(insns.len())];
        let loaded = tail.iter().position(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op1_kind() == OpKind::Memory
                && i.memory_base() == r
                && i.memory_displacement64() == 0
        });
        let Some(at) = loaded else { continue };
        let index = tail[at].op0_register();
        let dispatches = tail[at + 1..].iter().take(3).any(|i| {
            i.mnemonic() == Mnemonic::Movsxd
                && i.memory_index_scale() == 4
                && i.memory_index() == index
        });
        if dispatches && let Some(d) = disp(lea) {
            found.insert(d);
        }
    }
    let found: Vec<usize> = found.into_iter().collect();
    one_of("the request body's tag", &found)
}

fn request_layout(insns: &[Instruction]) -> Result<RequestLayout> {
    let (method, url) = method_and_url(insns)?;
    let body = body_tag(insns)?;
    let mut fields = [
        (0, HEADER_MAP),
        (body, 8 + BYTES),
        (url, STRING),
        (method, STRING),
    ];
    fields.sort_unstable();
    if fields.windows(2).any(|w| w[0].0 + w[0].1 > w[1].0) || fields.iter().any(|f| f.0 % 8 != 0) {
        bail!("body {body:#x}, URL {url:#x} and method {method:#x} overlap");
    }
    Ok(RequestLayout { body, url, method })
}

/// `(boxer, poll vtable)`: the small callee of `send` that loads a future's
/// vtable with `lea rdx`.
fn future_vtable(image: &Image<'_>, send: &[Instruction]) -> Result<(u64, u64)> {
    let mut found = BTreeSet::new();
    for (_, callee) in direct_calls(send) {
        let Some((start, _)) = image
            .function(callee)
            .filter(|&(s, e)| s == callee && e - s <= 0x80)
        else {
            continue;
        };
        let Ok((_, _, insns)) = function_code(image, "a callee of send", start) else {
            continue;
        };
        for (reg, vtable) in rip_leas(&insns) {
            let words: Option<Vec<u64>> = (0..4).map(|k| image.qword(vtable + 8 * k)).collect();
            if reg == Register::RDX
                && let Some([drop, size, align, poll]) =
                    words.as_deref().map(|w| [w[0], w[1], w[2], w[3]])
                && is_start(image, drop)
                && is_start(image, poll)
                && (0x100..=0x4000).contains(&size)
                && align == 8
            {
                found.insert((start, vtable));
            }
        }
    }
    let found: Vec<(u64, u64)> = found.into_iter().collect();
    one_of("the future send boxes", &found)
}

/// The response's layout, from the size `poll` copies into its result.
fn response_layout(insns: &[Instruction]) -> Result<ResponseLayout> {
    let result = insns
        .iter()
        .take(24)
        .find(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op_count() == 2
                && i.op1_kind() == OpKind::Register
                && i.op1_register() == Register::RCX
                && i.op0_kind() == OpKind::Register
        })
        .map(Instruction::op0_register)
        .context("poll keeps no copy of its result pointer")?;
    let copies: Vec<u64> = insns
        .windows(4)
        .filter_map(|w| {
            let size = w.iter().find_map(|i| {
                (i.mnemonic() == Mnemonic::Mov && i.op0_register() == Register::ECX)
                    .then(|| imm(i, 1))
                    .flatten()
            })?;
            let into = w.iter().any(|i| {
                i.mnemonic() == Mnemonic::Mov
                    && i.op0_register() == Register::RDI
                    && i.op1_kind() == OpKind::Register
                    && i.op1_register() == result
            });
            let rep = w
                .iter()
                .any(|i| i.code() == Code::Movsb_m8_m8 && i.has_rep_prefix());
            (into && rep).then_some(size)
        })
        .collect();
    let pending = insns.iter().any(|i| {
        i.op0_kind() == OpKind::Memory
            && i.memory_base() == result
            && i.memory_displacement64() == 0
            && imm(i, 1) == Some(PENDING)
    });
    match copies.as_slice() {
        [size, ..]
            if *size == BUFFERED_RESULT as u64 && copies.iter().all(|s| s == size) && pending =>
        {
            Ok(ResponseLayout {
                body: HEADER_MAP,
                status: HEADER_MAP + STRING,
            })
        }
        _ => bail!(
            "poll copies {copies:#x?} bytes into its result; only a {BUFFERED_RESULT:#x}-byte \
             result with the body in a Vec is read"
        ),
    }
}

/// `__rust_alloc`: the call in the box helper preceded by `mov edx, 8`,
/// whose first call is followed by `test al, al`.
fn allocator(image: &Image<'_>, boxer: u64) -> Result<u64> {
    let (_, _, insns) = function_code(image, "the future's box", boxer)?;
    let mut frontier: Vec<u64> = direct_calls(&insns).map(|(_, t)| t).collect();
    for _ in 0..2 {
        let mut found = BTreeSet::new();
        let mut next = Vec::new();
        for f in frontier {
            let Ok((_, end, code)) = function_code(image, "a callee", f) else {
                continue;
            };
            if end - f > 0x100 {
                continue;
            }
            for (k, t) in direct_calls(&code) {
                let aligned = code[k.saturating_sub(3)..k]
                    .iter()
                    .any(|i| i.op0_register() == Register::EDX && imm(i, 1) == Some(8));
                if aligned && checks_allocator(image, t) {
                    found.insert(t);
                }
                next.push(t);
            }
        }
        match found.len() {
            0 => frontier = next,
            1 => return Ok(found.into_iter().next().unwrap_or_default()),
            _ => bail!("{} candidates for __rust_alloc: {found:#x?}", found.len()),
        }
    }
    bail!("no __rust_alloc under the future's box")
}

fn checks_allocator(image: &Image<'_>, f: u64) -> bool {
    let Some(code) = image.read(f, 0x20) else {
        return false;
    };
    let insns = decode(code, f);
    insns
        .iter()
        .position(|i| i.mnemonic() == Mnemonic::Call)
        .and_then(|k| insns.get(k + 1))
        .is_some_and(|t| {
            t.mnemonic() == Mnemonic::Test
                && t.op0_register() == Register::AL
                && t.op1_register() == Register::AL
        })
}

/// The allocator static a shim hands its implementation in `rcx`.
fn allocator_static(image: &Image<'_>, shim: u64) -> Option<u64> {
    let insns = decode(image.read(shim, 0x40)?, shim);
    rip_leas(&insns)
        .find(|(r, _)| *r == Register::RCX)
        .map(|(_, t)| t)
}

/// `__rust_dealloc`: the other shim that loads `alloc`'s static and counts
/// down with `lock sub`.
fn deallocator(image: &Image<'_>, alloc: u64) -> Result<u64> {
    let global = allocator_static(image, alloc).context("__rust_alloc loads no allocator")?;
    // The shims are next to each other.
    let mut found = BTreeSet::new();
    let (lo, hi) = (alloc.saturating_sub(0x1000), alloc + 0x1000);
    let Some(code) = image.read(lo, usize::try_from(hi - lo)?) else {
        bail!("the code around __rust_alloc is unmapped");
    };
    for (k, _) in code
        .windows(3)
        .enumerate()
        .filter(|(_, w)| w[0] == 0x48 && w[1] == 0x8d && w[2] == 0x0d)
    {
        let at = lo + k as u64;
        let Some((start, end)) = image.function(at) else {
            continue;
        };
        if start == alloc
            || allocator_static(image, start) != Some(global)
            || checks_allocator(image, start)
        {
            continue;
        }
        let Some(body) = image.read(start, usize::try_from(end - start)?) else {
            continue;
        };
        let counts_down = decode(body, start)
            .iter()
            .any(|i| i.has_lock_prefix() && i.mnemonic() == Mnemonic::Sub);
        if counts_down {
            found.insert(start);
        }
    }
    let found: Vec<u64> = found.into_iter().collect();
    one_of("__rust_dealloc", &found)
}

/// Every aligned word in the data sections that holds one of `targets`, as
/// `(slot, target)`.
fn pointers_into(image: &Image<'_>, targets: &[u64]) -> Vec<(u64, u64)> {
    let set: BTreeSet<u64> = targets.iter().copied().collect();
    let mut out = Vec::new();
    for (start, end) in image.data() {
        let Some(bytes) = usize::try_from(end - start)
            .ok()
            .and_then(|n| image.read(start, n & !7))
        else {
            continue;
        };
        for (k, w) in bytes.chunks_exact(8).enumerate() {
            let v = u64::from_le_bytes(w.try_into().unwrap_or_default());
            if set.contains(&v) {
                out.push((start + 8 * k as u64, v));
            }
        }
    }
    out
}

/// The slots, among `slots`, that are a vtable's first method: a drop, a
/// size within `sizes` and an alignment before them.
fn method_slots(
    image: &Image<'_>,
    slots: &[u64],
    offset: u64,
    sizes: std::ops::RangeInclusive<u64>,
) -> Vec<u64> {
    slots
        .iter()
        .copied()
        .filter(|&slot| {
            let vtable = slot - offset;
            matches!(
                (image.qword(vtable), image.qword(vtable + 8), image.qword(vtable + 16)),
                (Some(drop), Some(size), Some(align))
                    if is_start(image, drop) && sizes.contains(&size) && align.is_power_of_two() && align <= 64
            )
        })
        .collect()
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

/// Find the REST transport.
pub fn transport(image: &Image<'_>) -> Result<Transport> {
    let anchor = one_of("the transport's string", &image.find(TRANSPORT_ANCHOR))?;
    let mut sends: Vec<u64> = image
        .leas_into(&[anchor])
        .into_iter()
        .filter_map(|(lea, _)| image.function(lea).map(|f| f.0))
        .collect();
    sends.dedup();
    let send = one_of("ReqwestTransport::send", &sends)?;
    let (_, _, insns) = function_code(image, "send", send)?;
    let request = request_layout(&insns)?;
    let (boxer, vtable) = future_vtable(image, &insns)?;
    let poll = image
        .qword(vtable + 24)
        .context("the future's vtable is unmapped")?;
    let (_, _, poll_insns) = function_code(image, "poll", poll)?;
    let response = response_layout(&poll_insns)?;
    let slots: Vec<u64> = pointers_into(image, &[poll])
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    let mut poll_slots = method_slots(image, &slots, 24, 0x100..=0x4000);
    if !poll_slots.contains(&(vtable + 24)) {
        poll_slots.push(vtable + 24);
    }
    poll_slots.sort_unstable();
    let alloc = allocator(image, boxer)?;
    let dealloc = deallocator(image, alloc)?;
    Ok(Transport {
        send,
        request,
        poll,
        poll_slots,
        response,
        alloc,
        dealloc,
    })
}

/// A registry entry at `slot`: an upper-case name of 1 to 64 bytes and a
/// handler that starts a function.
fn entry(image: &Image<'_>, slot: u64) -> Option<(String, u64)> {
    let (ptr, len, handler) = (
        image.qword(slot)?,
        image.qword(slot + 8)?,
        image.qword(slot + 16)?,
    );
    if !(1..=64).contains(&len) || !is_start(image, handler) {
        return None;
    }
    let name = image.read(ptr, usize::try_from(len).ok()?)?;
    name.iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
        .then(|| (String::from_utf8_lossy(name).into_owned(), handler))
}

/// A domain descriptor at `at`: a lower-case name, an id, and a slice of
/// entries, or an empty one (a dangling pointer, 8, and 0).
fn descriptor(image: &Image<'_>, at: u64) -> Option<(u64, u64)> {
    let w: Vec<u64> = (0..5)
        .map(|k| image.qword(at + 8 * k))
        .collect::<Option<_>>()?;
    let (name, len, id, slice, count) = (w[0], w[1], w[2], w[3], w[4]);
    if !(1..=64).contains(&len) || id > 0x1000 || count > 0x400 {
        return None;
    }
    let text = image.read(name, usize::try_from(len).ok()?)?;
    if !text
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
    {
        return None;
    }
    match count {
        0 => (slice == 8).then_some((slice, 0)),
        n => (0..n)
            .all(|k| entry(image, slice + ENTRY * k).is_some())
            .then_some((slice, n)),
    }
}

/// Every entry of the registry whose `messages` domain holds the entry at
/// `seed`.
fn registry(image: &Image<'_>, seed: u64) -> Result<Vec<(u64, String, u64)>> {
    let mut first = seed;
    while entry(image, first - ENTRY).is_some() {
        first -= ENTRY;
    }
    let holders: Vec<u64> = pointers_into(image, &[first])
        .into_iter()
        .map(|(s, _)| s - 24)
        .filter(|&d| descriptor(image, d).is_some())
        .collect();
    let holder = one_of("the descriptor of the seed's slice", &holders)?;
    let mut at = holder;
    while descriptor(image, at - DESCRIPTOR).is_some() {
        at -= DESCRIPTOR;
    }
    let mut out = Vec::new();
    while let Some((slice, count)) = descriptor(image, at) {
        for k in 0..count {
            let slot = slice + ENTRY * k;
            let (name, handler) = entry(image, slot).context("an entry the walk read went away")?;
            out.push((slot + 16, name, handler));
        }
        at += DESCRIPTOR;
    }
    Ok(out)
}

/// `start_send` of the sink the adopting connect loads.
fn sink(image: &Image<'_>) -> Result<(u64, Vec<u64>)> {
    let anchor = one_of("the adopt string", &image.find(ADOPT_ANCHOR))?;
    let panics = image.find(START_SEND_PANIC);
    if panics.is_empty() {
        bail!("no `this.state.is_none()` assertion");
    }
    let mut opens: Vec<u64> = image
        .leas_into(&[anchor])
        .into_iter()
        .filter_map(|(lea, _)| image.function(lea).map(|f| f.0))
        .collect();
    opens.dedup();
    let open = one_of("the adopting connect", &opens)?;
    let (_, _, insns) = function_code(image, "the adopting connect", open)?;
    let mut found = BTreeSet::new();
    for (_, vtable) in rip_leas(&insns) {
        let w: Option<Vec<u64>> = (0..7).map(|k| image.qword(vtable + 8 * k)).collect();
        let Some(w) = w else { continue };
        let methods = [w[0], w[3], w[4], w[5], w[6]];
        if !(0x20..=0x2000).contains(&w[1])
            || !w[2].is_power_of_two()
            || !methods.iter().all(|&m| is_start(image, m))
        {
            continue;
        }
        let start_send = w[4];
        let Ok((_, _, code)) = function_code(image, "start_send", start_send) else {
            continue;
        };
        if rip_leas(&code).any(|(_, t)| panics.contains(&t)) {
            found.insert((start_send, vtable));
        }
    }
    let found: Vec<(u64, u64)> = found.into_iter().collect();
    let (start_send, vtable) = one_of("the socket sink", &found)?;
    let slots: Vec<u64> = pointers_into(image, &[start_send])
        .into_iter()
        .map(|(s, _)| s)
        .collect();
    let mut slots = method_slots(image, &slots, START_SEND_SLOT, 0x20..=0x2000);
    if !slots.contains(&(vtable + START_SEND_SLOT)) {
        slots.push(vtable + START_SEND_SLOT);
    }
    slots.sort_unstable();
    Ok((start_send, slots))
}

/// Find the gateway's handlers and sink.
pub fn gateway(image: &Image<'_>) -> Result<Gateway> {
    let strings = image.find(SEED);
    let seeds: Vec<u64> = pointers_into(image, &strings)
        .into_iter()
        .map(|(s, _)| s)
        .filter(|&s| image.qword(s + 8) == Some(SEED.len() as u64) && entry(image, s).is_some())
        .collect();
    if seeds.is_empty() {
        bail!("no registry entry names MESSAGE_CREATE");
    }
    let mut by_name: BTreeMap<String, Handler> = BTreeMap::new();
    for seed in seeds {
        for (slot, name, handler) in registry(image, seed)? {
            let h = by_name.entry(name.clone()).or_insert_with(|| Handler {
                name: name.clone(),
                handler,
                slots: Vec::new(),
            });
            if h.handler != handler {
                bail!("{name} has the handlers {:#x} and {handler:#x}", h.handler);
            }
            if !h.slots.contains(&slot) {
                h.slots.push(slot);
            }
        }
    }
    let (start_send, start_send_slots) = sink(image)?;
    Ok(Gateway {
        handlers: by_name.into_values().collect(),
        start_send,
        start_send_slots,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_2208() -> Option<Vec<u8>> {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return None;
        };
        Some(std::fs::read(path).unwrap())
    }

    #[test]
    fn finds_the_transport_on_build_2208() {
        let Some(bytes) = build_2208() else { return };
        let image = Image::parse(&bytes).unwrap();
        let t = transport(&image).unwrap();
        eprintln!("{t:#x?}");
        let rva = |va: u64| va - image.base();
        assert_eq!(rva(t.send), 0x7923400);
        assert_eq!(
            t.request,
            RequestLayout {
                body: 0x60,
                url: 0x98,
                method: 0xc0
            }
        );
        assert_eq!(rva(t.poll), 0xff32050);
        assert_eq!(
            t.poll_slots.iter().map(|&s| rva(s)).collect::<Vec<_>>(),
            [0x17397a98]
        );
        assert_eq!(
            t.response,
            ResponseLayout {
                body: 0x60,
                status: 0x78
            }
        );
        assert_eq!((rva(t.alloc), rva(t.dealloc)), (0x456d100, 0x456d150));
    }

    #[test]
    fn finds_the_gateway_on_build_2208() {
        let Some(bytes) = build_2208() else { return };
        let image = Image::parse(&bytes).unwrap();
        let g = gateway(&image).unwrap();
        let rva = |va: u64| va - image.base();
        assert_eq!(g.handlers.len(), 89);
        let create = g
            .handlers
            .iter()
            .find(|h| h.name == "MESSAGE_CREATE")
            .unwrap();
        assert_eq!(rva(create.handler), 0x7b4a590);
        assert_eq!(
            create.slots.iter().map(|&s| rva(s)).collect::<Vec<_>>(),
            [0x163451c8, 0x1688b618]
        );
        assert!(g.handlers.iter().all(|h| h.slots.len() == 2));
        assert_eq!(rva(g.start_send), 0xe6981d0);
        assert_eq!(
            g.start_send_slots
                .iter()
                .map(|&s| rva(s))
                .collect::<Vec<_>>(),
            [0x17381510]
        );
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
    fn the_layout_reads_off_the_copies_before_the_headers_move() {
        // `send` on build 2208, from the copies to the headers' move.
        let code = [
            0x48, 0x8b, 0x4d, 0x30, // mov rcx, [rbp+0x30]
            0x48, 0x8b, 0x81, 0xd0, 0x00, 0x00, 0x00, // mov rax, [rcx+0xd0]
            0x4c, 0x8d, 0x45, 0x00, // lea r8, [rbp]
            0x49, 0x89, 0x40, 0x10, // mov [r8+0x10], rax
            0x0f, 0x10, 0x81, 0xc0, 0x00, 0x00, 0x00, // movups xmm0, [rcx+0xc0]
            0x41, 0x0f, 0x29, 0x00, // movaps [r8], xmm0
            0x48, 0x8b, 0x81, 0xa8, 0x00, 0x00, 0x00, // mov rax, [rcx+0xa8]
            0x4c, 0x8d, 0x4d, 0x20, // lea r9, [rbp+0x20]
            0x49, 0x89, 0x41, 0x10, // mov [r9+0x10], rax
            0x0f, 0x10, 0x81, 0x98, 0x00, 0x00, 0x00, // movups xmm0, [rcx+0x98]
            0x41, 0x0f, 0x29, 0x01, // movaps [r9], xmm0
            0xe8, 0x00, 0x00, 0x00, 0x00, // call
            0x6a, 0x60, // push 0x60
            0x59, // pop rcx
            0xf3, 0xa4, // rep movsb
            0x48, 0x8d, 0x5a, 0x60, // lea rbx, [rdx+0x60]
            0x48, 0x8b, 0x03, // mov rax, [rbx]
            0x48, 0x8d, 0x0d, 0x00, 0x00, 0x00, 0x00, // lea rcx, [rip]
            0x48, 0x63, 0x04, 0x81, // movsxd rax, [rcx+rax*4]
        ];
        let insns = decode(&code, 0x1000);
        assert_eq!(
            request_layout(&insns).unwrap(),
            RequestLayout {
                body: 0x60,
                url: 0x98,
                method: 0xc0
            }
        );
    }
}
