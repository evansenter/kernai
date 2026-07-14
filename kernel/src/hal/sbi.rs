//! SBI calls (the kernel's only way to touch the world below S-mode).
//! We run on whatever OpenSBI QEMU bundles — never vendored, never M-mode.

use core::arch::asm;

/// One `ecall` to the SBI firmware, binary-interface per the SBI spec:
/// EID in a7, FID in a6, args in a0/a1; returns (error, value) in (a0, a1).
#[inline]
fn sbi_call(eid: usize, fid: usize, arg0: usize, arg1: usize) -> (isize, usize) {
    let error: isize;
    let value: usize;
    // SAFETY: `ecall` from S-mode transfers to the SBI firmware, which per
    // the SBI spec preserves all registers except a0/a1 (declared as
    // outputs). No memory is passed, so no aliasing or validity obligations.
    unsafe {
        asm!(
            "ecall",
            in("a7") eid,
            in("a6") fid,
            inlateout("a0") arg0 => error,
            inlateout("a1") arg1 => value,
        );
    }
    (error, value)
}

/// Legacy console putchar (EID 0x01). Deliberately dumb and universal;
/// DBCN batching is a deliberate non-feature until the serial layer has
/// proven itself (DECISIONS.md).
pub fn console_putchar(byte: u8) {
    sbi_call(0x01, 0, byte as usize, 0);
}

/// System Reset extension (EID "SRST"): shutdown, never returns.
/// `failure` selects the SBI reset reason (0 = no reason, 1 = system failure).
pub fn shutdown(failure: bool) -> ! {
    sbi_call(0x5352_5354, 0, 0, failure as usize);
    // SRST can only fail if the platform lacks a reset method; QEMU virt
    // has one. Spin rather than return a lie.
    loop {
        core::hint::spin_loop();
    }
}
