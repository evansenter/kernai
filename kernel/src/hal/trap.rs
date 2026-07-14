//! Supervisor trap vector: full trap-frame save/restore around a call into
//! the safe handler (`crate::traps::handle`), for traps arriving from both
//! S-mode (kernel) and U-mode (payloads).
//!
//! sscratch convention: 0 while the kernel runs (trap keeps the current
//! kernel sp); the kernel trap-stack top while a payload runs (trap swaps
//! onto it). The Rust shim re-arms sscratch on every return according to
//! the privilege the frame resumes into (sstatus.SPP).

use core::arch::global_asm;

use super::{boot, csr};
use crate::traps::TrapFrame;

const SSTATUS_SPP: u64 = 1 << 8;
const SSTATUS_SPIE: u64 = 1 << 5;

// Layout must match crate::traps::TrapFrame exactly:
//   regs[31] (x1..x31 at offset (n-1)*8) | sepc @ 248 | sstatus @ 256.
// 272 = 264 rounded up to keep sp 16-aligned. Unlike pre-M3, sp (x2) is
// restored FROM THE FRAME (last load), so the handler can redirect the
// resumed context (kill a payload, re-enter the idle loop) by rewriting
// frame.{sepc, regs[1], sstatus}.
global_asm!(
    r#"
    .section .text
    .align 4
    .globl __trap_vector
__trap_vector:
    csrrw sp, sscratch, sp      // swap: from U -> sp = trap stack, sscratch = user sp
    bnez  sp, 1f                // nonzero => trap from U-mode
    csrrw sp, sscratch, sp      // from S: undo (sscratch back to 0, sp restored)
1:
    addi sp, sp, -272
    sd   x1,   0(sp)
    sd   x3,  16(sp)
    sd   x4,  24(sp)
    sd   x5,  32(sp)
    sd   x6,  40(sp)
    sd   x7,  48(sp)
    sd   x8,  56(sp)
    sd   x9,  64(sp)
    sd   x10, 72(sp)
    sd   x11, 80(sp)
    sd   x12, 88(sp)
    sd   x13, 96(sp)
    sd   x14, 104(sp)
    sd   x15, 112(sp)
    sd   x16, 120(sp)
    sd   x17, 128(sp)
    sd   x18, 136(sp)
    sd   x19, 144(sp)
    sd   x20, 152(sp)
    sd   x21, 160(sp)
    sd   x22, 168(sp)
    sd   x23, 176(sp)
    sd   x24, 184(sp)
    sd   x25, 192(sp)
    sd   x26, 200(sp)
    sd   x27, 208(sp)
    sd   x28, 216(sp)
    sd   x29, 224(sp)
    sd   x30, 232(sp)
    sd   x31, 240(sp)
    csrr t0, sstatus            // classify by the trapped privilege (SPP),
    sd   t0, 256(sp)            // never by an overloaded sscratch==0 sentinel
    csrr t2, sepc
    sd   t2, 248(sp)
    andi t1, t0, 0x100          // sstatus.SPP (bit 8): 1 = trap from S-mode
    bnez t1, 2f
    csrr t1, sscratch           // from U: interrupted user sp (may legitimately be 0)
    j    3f
2:  addi t1, sp, 272            // from S: pre-trap kernel sp
3:  sd   t1, 8(sp)              // x2 slot (t0/t1/t2 = x5/x7/x6, already saved)
    csrw sscratch, zero         // in the kernel now: S-mode convention
    mv   a0, sp
    call __kernai_trap
    ld   t0, 248(sp)
    csrw sepc, t0
    ld   t0, 256(sp)
    csrw sstatus, t0
    ld   x1,   0(sp)
    ld   x3,  16(sp)
    ld   x4,  24(sp)
    ld   x5,  32(sp)
    ld   x6,  40(sp)
    ld   x7,  48(sp)
    ld   x8,  56(sp)
    ld   x9,  64(sp)
    ld   x10, 72(sp)
    ld   x11, 80(sp)
    ld   x12, 88(sp)
    ld   x13, 96(sp)
    ld   x14, 104(sp)
    ld   x15, 112(sp)
    ld   x16, 120(sp)
    ld   x17, 128(sp)
    ld   x18, 136(sp)
    ld   x19, 144(sp)
    ld   x20, 152(sp)
    ld   x21, 160(sp)
    ld   x22, 168(sp)
    ld   x23, 176(sp)
    ld   x24, 184(sp)
    ld   x25, 192(sp)
    ld   x26, 200(sp)
    ld   x27, 208(sp)
    ld   x28, 216(sp)
    ld   x29, 224(sp)
    ld   x30, 232(sp)
    ld   x31, 240(sp)
    ld   sp,   8(sp)            // x2 from the frame, last — enables redirects
    sret
    "#
);

// SAFETY: __trap_vector is defined by the global_asm above in this file; it
// is only ever *named* (address taken for stvec), never called from Rust.
unsafe extern "C" {
    fn __trap_vector();
}

/// Point stvec at the trap vector and establish the S-mode sscratch
/// convention. Must run before interrupts are enabled.
pub fn init() {
    csr::write_sscratch(0);
    csr::write_stvec(__trap_vector as *const () as usize);
}

// SAFETY: called only from __trap_vector with a0 pointing at the TrapFrame
// just saved on this stack; the frame is exclusively borrowed for the call.
// Unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn __kernai_trap(frame: &mut TrapFrame) {
    crate::traps::handle(frame);
    // Re-arm sscratch for wherever this frame resumes: trap-stack top if
    // returning to U-mode (SPP=0), 0 if staying in S-mode.
    if frame.sstatus & SSTATUS_SPP == 0 {
        csr::write_sscratch(boot::trap_stack_top());
    } else {
        csr::write_sscratch(0);
    }
}

/// First entry into a freshly loaded payload: drop to U-mode at `entry`.
/// The payload's own `_start` sets its stack; registers are not scrubbed
/// (no kernel secrets exist yet — revisit with M5 isolation).
pub fn enter_user(entry: usize) -> ! {
    let trap_stack = boot::trap_stack_top();
    // SAFETY: arming sscratch and the sret must be atomic w.r.t. interrupts.
    // enter_user runs in S-mode with interrupts enabled (the scheduler is
    // reached with SPIE=1). If a timer fired between the sscratch write and
    // the sret, that trap — seeing sscratch != 0 — would be misclassified as
    // from-U and __kernai_trap would reset sscratch to 0, so the payload
    // would enter with a desynced sscratch and its next trap would build the
    // kernel frame on the payload's own stack (a privilege-boundary break).
    // So: clear sstatus.SIE first (bit 1); sret restores SIE from SPIE=1,
    // re-enabling interrupts atomically on entry to U-mode. SPP=0 selects
    // U-mode. `entry` was validated by the loader to lie in the arena.
    unsafe {
        core::arch::asm!(
            "csrci sstatus, 2",         // clear sstatus.SIE: no trap in the window below
            "csrw sscratch, {tstack}",  // arm the U-mode trap stack
            "csrw sepc, {entry}",
            "csrc sstatus, {spp}",      // SPP = 0 (return to U-mode)
            "csrs sstatus, {spie}",     // SPIE = 1 (interrupts on after sret)
            "sret",
            tstack = in(reg) trap_stack,
            entry = in(reg) entry,
            spp = in(reg) SSTATUS_SPP,
            spie = in(reg) SSTATUS_SPIE,
            options(noreturn),
        )
    }
}

/// Deliberately execute an illegal instruction (M2 acceptance 3; later the
/// E1 seeded-bug machinery grows richer injectors). csrrw to the read-only
/// `cycle` counter is illegal by the privileged spec, and its encoding
/// decodes into meaningful fields for the fault report.
pub fn trigger_illegal_instruction() -> ! {
    // SAFETY: this instruction never retires — it always raises
    // illegal-instruction; the trap handler emits the fault report and shuts
    // down. options(noreturn) states that contract, and omitting `nomem`
    // keeps the handler's memory effects visible to the compiler.
    unsafe { core::arch::asm!(".word 0xc0001073", options(noreturn)) };
}
