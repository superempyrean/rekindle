//! The detour itself: prologue decoding, the trampoline and the jump.

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction};

use super::mem::{self, Block, Protection};
use super::{Detour, InlineDetour};

/// Size of `jmp rel32`.
const NEAR_JMP_LEN: usize = 5;

/// Size of `jmp qword ptr [rip]` followed by its 8-byte target.
const ABS_JMP_LEN: usize = 14;

/// The longest prologue a detour moves into a trampoline. The `# Safety`
/// section of [`Detour::install`] names this number.
const MAX_STOLEN: usize = 24;

/// A trampoline holds the moved prologue and the jump back.
const TRAMPOLINE_SIZE: usize = MAX_STOLEN + ABS_JMP_LEN;

/// `jmp rel32` at `from` to `to`, if `to` is in range.
pub(super) fn near_jmp(from: usize, to: usize) -> Option<[u8; NEAR_JMP_LEN]> {
    let displacement = (to as i64) - (from as i64 + NEAR_JMP_LEN as i64);
    let displacement = i32::try_from(displacement).ok()?;
    let mut out = [0xE9, 0, 0, 0, 0];
    out[1..].copy_from_slice(&displacement.to_le_bytes());
    Some(out)
}

/// `jmp qword ptr [rip+0]` followed by `to`: the displacement of 0 names the
/// qword right after the instruction.
fn abs_jmp(to: usize) -> [u8; ABS_JMP_LEN] {
    let mut out = [0u8; ABS_JMP_LEN];
    out[..2].copy_from_slice(&[0xFF, 0x25]);
    out[6..].copy_from_slice(&(to as u64).to_le_bytes());
    out
}

/// The length of the instruction at the start of `code`, if it means the
/// same in a trampoline: no RIP-relative operand and no control flow.
fn relocatable_len(code: &[u8]) -> Option<usize> {
    let instruction = Decoder::new(64, code, DecoderOptions::NONE).decode();
    if instruction.is_invalid() || !is_relocatable(&instruction) {
        return None;
    }
    Some(instruction.len())
}

fn is_relocatable(instruction: &Instruction) -> bool {
    !instruction.is_ip_rel_memory_operand()
        && !matches!(
            instruction.flow_control(),
            FlowControl::Call
                | FlowControl::IndirectCall
                | FlowControl::UnconditionalBranch
                | FlowControl::ConditionalBranch
                | FlowControl::IndirectBranch
                | FlowControl::XbeginXabortXend
                | FlowControl::Return
                | FlowControl::Interrupt
                | FlowControl::Exception
        )
}

/// How many bytes of whole, relocatable instructions cover the first
/// `needed` bytes of `code`, or `None` if they can't be moved as they are.
fn prologue_len(code: &[u8], needed: usize) -> Option<usize> {
    let mut total = 0;
    while total < needed {
        total += relocatable_len(code.get(total..)?)?;
        if total > MAX_STOLEN {
            return None;
        }
    }
    Some(total)
}

/// Write `bytes` over code at `addr`, making its pages writable for the
/// write.
///
/// # Safety
///
/// `addr` is code of this process that no thread runs while it is written.
unsafe fn patch(addr: usize, bytes: &[u8]) -> Result<()> {
    let ptr = addr as *mut u8;
    // SAFETY: the pages are mapped code of this process, and making them
    // writable can't break code that only runs them.
    unsafe { mem::protect(ptr, bytes.len(), Protection::ReadWriteExecute) }?;
    // SAFETY: the pages are writable now, and the caller guarantees no
    // thread runs these bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len()) };
    // The bytes are in place. If the pages can't be made read-only again,
    // they stay writable, which changes nothing the hook relies on.
    // SAFETY: as for the first call; nothing writes to them from here on.
    let _ = unsafe { mem::protect(ptr, bytes.len(), Protection::ReadExecute) };
    mem::flush(ptr, bytes.len());
    Ok(())
}

impl Detour for InlineDetour {
    unsafe fn install(&self, target: usize, hook: usize) -> Result<Installed> {
        // SAFETY: forwarded from the caller.
        unsafe { Installed::new(target, hook) }
    }
}

/// An installed hook. Dropping it writes the original bytes back.
#[derive(Debug)]
pub struct Installed {
    target: usize,
    original: [u8; NEAR_JMP_LEN],
    /// `None` once unhooked.
    trampoline: Option<Block>,
    /// The far-jump thunk, for a hook out of `rel32` reach of the target.
    thunk: Option<Block>,
}

impl Installed {
    /// # Safety
    ///
    /// As for [`Detour::install`].
    unsafe fn new(target: usize, hook: usize) -> Result<Self> {
        if target == 0 || hook == 0 {
            bail!("target and hook addresses must be non-null");
        }
        // SAFETY: the caller guarantees `target` is followed by MAX_STOLEN
        // bytes of readable code.
        let code = unsafe { std::slice::from_raw_parts(target as *const u8, MAX_STOLEN) };
        // The whole prologue is decoded before anything is written.
        let stolen = prologue_len(code, NEAR_JMP_LEN)
            .with_context(|| format!("can't move the prologue at {target:#x}, not hooking it"))?;
        let mut original = [0u8; NEAR_JMP_LEN];
        original.copy_from_slice(&code[..NEAR_JMP_LEN]);

        let mut trampoline = Block::alloc(TRAMPOLINE_SIZE).context("allocating a trampoline")?;
        trampoline.write(0, &code[..stolen]);
        trampoline.write(stolen, &abs_jmp(target + stolen));

        let (jump, thunk) = if let Some(jump) = near_jmp(target, hook) {
            (jump, None)
        } else {
            // The hook is in another module, out of reach of a jmp rel32:
            // jump to a thunk within reach that makes the absolute jump.
            let mut thunk = Block::alloc_near(target, ABS_JMP_LEN)
                .with_context(|| format!("placing a far-jump thunk near {target:#x}"))?;
            thunk.write(0, &abs_jmp(hook));
            let jump = near_jmp(target, thunk.addr())
                .expect("Block::alloc_near returns memory in rel32 reach of the target");
            (jump, Some(thunk))
        };
        // SAFETY: the caller guarantees `target` is code no thread runs
        // while it is patched.
        unsafe { patch(target, &jump) }?;
        Ok(Self {
            target,
            original,
            trampoline: Some(trampoline),
            thunk,
        })
    }

    /// The address of the moved prologue, followed by a jump back into the
    /// original function: cast it to a function pointer of the target's
    /// signature to call the original. 0 once unhooked.
    pub fn trampoline(&self) -> usize {
        self.trampoline.as_ref().map_or(0, Block::addr)
    }

    /// Write the original bytes back and free the trampoline. Does nothing
    /// once unhooked.
    pub fn unhook(&mut self) -> Result<()> {
        if self.trampoline.is_none() {
            return Ok(());
        }
        // SAFETY: `target` is the code `new` patched, and the contract of
        // `Detour::install` holds while the hook is removed.
        unsafe { patch(self.target, &self.original) }?;
        self.trampoline = None;
        self.thunk = None;
        Ok(())
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        if self.unhook().is_err() {
            // The jump is still in place and still leads to the thunk and
            // the trampoline, so they must never be freed.
            std::mem::forget(self.trampoline.take());
            std::mem::forget(self.thunk.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_jmp_encodes_a_relative_branch() {
        let jump = near_jmp(0x1000, 0x2000).unwrap();
        assert_eq!(jump[0], 0xE9);
        let displacement = i32::from_le_bytes(jump[1..].try_into().unwrap());
        assert_eq!(
            0x1000_i64 + NEAR_JMP_LEN as i64 + i64::from(displacement),
            0x2000
        );
        assert!(near_jmp(0, i32::MAX as usize + NEAR_JMP_LEN + 1).is_none());
    }

    #[test]
    fn abs_jmp_is_fourteen_bytes_and_ends_with_the_address() {
        let jump = abs_jmp(0x1234_5678_9abc_def0);
        assert_eq!(jump.len(), ABS_JMP_LEN);
        assert_eq!(jump[..6], [0xFF, 0x25, 0, 0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(jump[6..].try_into().unwrap()),
            0x1234_5678_9abc_def0
        );
    }

    #[test]
    fn decodes_common_prologue_instructions() {
        assert_eq!(relocatable_len(&[0x55]), Some(1)); // push rbp
        assert_eq!(relocatable_len(&[0x48, 0x83, 0xEC, 0x28]), Some(4)); // sub rsp, 0x28
        assert_eq!(
            relocatable_len(&[0x48, 0x8D, 0x81, 0xE8, 0x03, 0x00, 0x00]),
            Some(7)
        ); // lea rax, [rcx+1000]
        assert_eq!(relocatable_len(&[0x48, 0x89, 0x5C, 0x24, 0x08]), Some(5)); // mov [rsp+8], rbx
        assert_eq!(relocatable_len(&[0xF3, 0x0F, 0x1E, 0xFA]), Some(4)); // endbr64
        assert_eq!(
            relocatable_len(&[0x48, 0xB8, 1, 2, 3, 4, 5, 6, 7, 8]),
            Some(10)
        ); // mov rax, imm64
    }

    #[test]
    fn refuses_truncated_instructions() {
        assert_eq!(relocatable_len(&[0x48, 0x83]), None);
        assert_eq!(relocatable_len(&[]), None);
        assert_eq!(relocatable_len(&[0x0F]), None);
    }

    #[test]
    fn refuses_prologues_it_cannot_relocate() {
        // A branch can't be copied into a trampoline without retargeting.
        assert_eq!(prologue_len(&[0xE9, 0, 0, 0, 0, 0], NEAR_JMP_LEN), None);
        // Nor can a conditional one: `test eax, eax; je +5` is relative to
        // where it runs.
        assert_eq!(
            prologue_len(&[0x85, 0xC0, 0x74, 0x05, 0x90, 0x90], NEAR_JMP_LEN),
            None
        );
        assert_eq!(prologue_len(&[0x0F, 0x84, 0, 0, 0, 0], NEAR_JMP_LEN), None);
        assert_eq!(
            prologue_len(&[0xE3, 0x02, 0x90, 0x90, 0x90], NEAR_JMP_LEN),
            None
        ); // jrcxz
        // A RIP-relative load needs its displacement fixed up.
        let rip_relative = [0x48, 0x8B, 0x05, 0x11, 0x22, 0x33, 0x44];
        assert_eq!(prologue_len(&rip_relative, NEAR_JMP_LEN), None);
        // Ordinary register work moves, in whole instructions only.
        let plain = [0x48, 0x89, 0x5C, 0x24, 0x08, 0x90];
        assert_eq!(prologue_len(&plain, NEAR_JMP_LEN), Some(5));
        assert_eq!(prologue_len(&plain, 4), Some(5));
    }

    #[test]
    fn refuses_a_prologue_longer_than_a_trampoline_holds() {
        // Five `nop dword [rax+rax+0]` of 5 bytes each: 25 bytes to cover 21.
        let nops = [0x0F, 0x1F, 0x44, 0x00, 0x00].repeat(5);
        assert_eq!(prologue_len(&nops, MAX_STOLEN - 3), None);
        assert_eq!(prologue_len(&nops, MAX_STOLEN - 4), Some(MAX_STOLEN - 4));
    }

    #[test]
    fn detour_round_trip() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CALLS: AtomicUsize = AtomicUsize::new(0);

        #[inline(never)]
        #[unsafe(no_mangle)]
        extern "C" fn rekindle_hook_target(value: u64) -> u64 {
            value.wrapping_mul(2).wrapping_add(1)
        }

        #[inline(never)]
        #[unsafe(no_mangle)]
        extern "C" fn rekindle_hook_replacement(value: u64) -> u64 {
            CALLS.fetch_add(1, Ordering::SeqCst);
            value.wrapping_add(1000)
        }

        let target = std::hint::black_box(rekindle_hook_target as *const ()) as usize;
        let replacement = rekindle_hook_replacement as *const () as usize;
        // SAFETY: `target` is the address of `rekindle_hook_target`, whose
        // type this is.
        let call: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(target) };

        assert_eq!(call(3), 7);

        // SAFETY: `rekindle_hook_target` is a function of this test binary
        // with the replacement's ABI, and only this thread runs it.
        let mut installed = unsafe { InlineDetour.install(target, replacement) }.unwrap();
        assert_ne!(installed.trampoline(), 0);
        assert_eq!(call(3), 1003);
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);

        installed.unhook().unwrap();
        assert_eq!(installed.trampoline(), 0);
        assert_eq!(call(3), 7);
    }
}
