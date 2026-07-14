#![no_std]
#![no_main]
//! The M3 acceptance payload: prove a U-mode program can talk to the kernel
//! and exit cleanly. `_start` (in sys) exits 0 when pmain returns.

// SAFETY: called exactly once by sys's `_start` asm; unmangled so the asm
// can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"hello from userspace");
}
