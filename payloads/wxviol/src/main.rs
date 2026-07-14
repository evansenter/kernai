#![no_std]
#![no_main]
//! M5 W^X fixture: a U-mode payload tries to write to its own code. The
//! loader maps text pages R|X (never W), so the store faults (store page
//! fault) — code cannot be rewritten. The kernel kills only this payload.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"trying to rewrite my own code...");
    // SAFETY(payload): deliberately illegal — .text is mapped read+execute,
    // not writable, so this store raises a store/AMO page fault.
    let code = pmain as *mut u8;
    unsafe { core::ptr::write_volatile(code, 0) };
    sys::write(b"if you see this, W^X failed");
}
