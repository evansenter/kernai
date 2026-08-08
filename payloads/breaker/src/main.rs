#![no_std]
#![no_main]
//! E1 stimulus: a bare `ebreak` from U-mode — scause 3 (breakpoint). Distinct
//! diagnosis from every other stimulus: the "fault" is an intentional trap
//! instruction, which the decoded instruction field makes obvious on the
//! structured surface and nothing does on the classic one.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"hitting a breakpoint instruction");
    // SAFETY: deliberately trapping — `ebreak` raises a breakpoint exception;
    // that trap is the test.
    unsafe { core::arch::asm!("ebreak") };
}
