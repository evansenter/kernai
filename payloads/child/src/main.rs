#![no_std]
#![no_main]
//! M4 spawned child. Runs after its parent (sequential pre-paging) with the
//! attenuated CapSet the kernel granted it ({write}); writes and exits 0.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"child running with attenuated caps");
}
