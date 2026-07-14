//! Supervisor trap vector: full trap-frame save/restore around a call into
//! the safe handler (`crate::traps::handle`).
//!
//! M3 seam: traps currently only arrive from S-mode (no U-mode yet), so the
//! vector stays on the kernel stack. U-mode entry will add an
//! sscratch-based stack swap here — this file, no new unsafe files.

use core::arch::global_asm;

use super::csr;
use crate::traps::TrapFrame;

// Layout must match crate::traps::TrapFrame exactly:
//   regs[31] (x1..x31 at offset (n-1)*8) | sepc @ 248 | sstatus @ 256.
// 272 = 264 rounded up to keep sp 16-aligned.
global_asm!(
    r#"
    .section .text
    .align 4
    .globl __trap_vector
__trap_vector:
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
    addi t0, sp, 272            // t0 (x5) already saved: reuse for pre-trap sp
    sd   t0, 8(sp)
    csrr t0, sepc
    sd   t0, 248(sp)
    csrr t0, sstatus
    sd   t0, 256(sp)
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
    addi sp, sp, 272            // x2 restored arithmetically, not from the frame
    sret
    "#
);

// SAFETY: __trap_vector is defined by the global_asm above in this file; it
// is only ever *named* (address taken for stvec), never called from Rust.
unsafe extern "C" {
    fn __trap_vector();
}

/// Point stvec at the trap vector. Must run before interrupts are enabled.
pub fn init() {
    csr::write_stvec(__trap_vector as *const () as usize);
}

// SAFETY: called only from __trap_vector with a0 pointing at the TrapFrame
// just saved on this stack; the frame is exclusively borrowed for the call.
// Unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn __kernai_trap(frame: &mut TrapFrame) {
    crate::traps::handle(frame);
}

/// Deliberately execute an illegal instruction (M2 acceptance 3; later the
/// E1 seeded-bug machinery grows richer injectors). csrrw to the read-only
/// `cycle` counter is illegal by the privileged spec, and its encoding
/// decodes into meaningful fields for the fault report.
pub fn trigger_illegal_instruction() -> ! {
    // SAFETY: this instruction never retires — it raises illegal-instruction
    // immediately; the trap handler emits the fault report and shuts down.
    unsafe { core::arch::asm!(".word 0xc0001073", options(nomem, nostack)) };
    unreachable!("illegal instruction fell through");
}
