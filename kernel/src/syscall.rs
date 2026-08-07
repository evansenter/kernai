#![forbid(unsafe_code)]
//! Syscall dispatch (ecall from U-mode). ABI v0: number in a7, args in
//! a0..a2, result in a0 (0/positive ok, negative errno). The surface is
//! deliberately tiny (RFC non-goal: no POSIX): exit/write/yield/spawn.
//!
//! Capability enforcement lives here (P10 seed): every capability-gated call
//! checks the current payload's CapSet before acting. Payload output is
//! tagged untrusted on the way out (P7 seed) in `payload::emit_output`.

use crate::payload::{self, Cap};
use crate::traps::TrapFrame;

pub const SYS_EXIT: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_YIELD: u64 = 2;
pub const SYS_SPAWN: u64 = 3;
pub const SYS_SNAPSHOT: u64 = 4;
pub const SYS_BLIT: u64 = 5;
/// Stream a raw framebuffer chunk out as a base64 `fbchunk` event (full-color
/// keyframes the host reassembles). Only wired in the `doom` feature build —
/// the default surface stays exit/write/yield/spawn/snapshot/blit.
#[cfg(feature = "doom")]
pub const SYS_FRAME: u64 = 6;
/// Pop one operator-input key byte (queued by the timer tick's serial drain);
/// EAGAIN when empty. The other half of the agentic loop: frames out via
/// blit/frame, keys in via getkey. `doom` feature build only.
#[cfg(feature = "doom")]
pub const SYS_GETKEY: u64 = 7;

// errno-style returns (negative). Kept few and structured.
pub const ENOSYS: isize = -1;
pub const ENOCAP: isize = -2;
pub const EFAULT: isize = -3;
pub const EINVAL: isize = -4;
pub const EAGAIN: isize = -5; // no free process/snapshot slot
pub const ENOMEM: isize = -6; // frame pool exhausted

/// What the trap handler should do after a syscall.
pub enum Outcome {
    /// Resume the same payload with this value in a0.
    Resume(isize),
    /// The payload is leaving the CPU (exited); hand control to the scheduler.
    Leave,
}

pub fn dispatch(frame: &mut TrapFrame) -> Outcome {
    let nr = frame.a7();
    let (a0, a1) = (frame.a0() as usize, frame.a1() as usize);
    match nr {
        SYS_EXIT => {
            payload::on_exit(a0);
            Outcome::Leave
        }
        SYS_WRITE => Outcome::Resume(sys_write(a0, a1)),
        SYS_YIELD => Outcome::Resume(payload::on_yield()),
        SYS_SPAWN => Outcome::Resume(payload::on_spawn(a0, a1 as u32)),
        SYS_SNAPSHOT => Outcome::Resume(payload::on_snapshot(frame)),
        SYS_BLIT => Outcome::Resume(payload::on_blit(a0, a1, frame.a2() as usize)),
        #[cfg(feature = "doom")]
        SYS_FRAME => Outcome::Resume(payload::on_frame_rgb(a0, a1, frame.a2() as usize)),
        #[cfg(feature = "doom")]
        SYS_GETKEY => Outcome::Resume(payload::on_getkey()),
        _ => Outcome::Resume(ENOSYS),
    }
}

/// write(ptr, len): emit up to WRITE_QUOTA payload bytes as an untrusted
/// output event. Requires Cap::WRITE. Returns bytes accepted, or errno.
fn sys_write(ptr: usize, len: usize) -> isize {
    if !payload::current_has(Cap::Write) {
        payload::emit_denied("write", "write");
        return ENOCAP;
    }
    // P3/P7 quota seed: a hostile payload cannot flood the operator's
    // context. Accept at most WRITE_QUOTA bytes per call.
    let take = len.min(payload::WRITE_QUOTA);
    match payload::emit_output(ptr, take) {
        Ok(()) => take as isize,
        Err(()) => EFAULT,
    }
}
