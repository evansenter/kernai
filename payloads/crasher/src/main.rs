#![no_std]
#![no_main]
//! The M3 fault payload: prove that a U-mode crash produces a structured
//! fault report and kills only the payload — the kernel must keep running.

// SAFETY: called exactly once by sys's `_start` asm; unmangled so the asm
// can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"about to touch an S-mode CSR from U-mode");
    // SAFETY: deliberately NOT safe — writing sstatus from U-mode raises
    // illegal-instruction (privilege too low). That trap is the test.
    unsafe { core::arch::asm!("csrw sstatus, zero") };
}
