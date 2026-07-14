#![no_std]
#![no_main]
//! M5 isolation fixture: a U-mode payload tries to read kernel memory
//! (0x80200000, in the kernel's supervisor gigapage). Pre-M5 this succeeded —
//! there was no MMU. With per-payload paging the kernel page is U=0, so the
//! load faults (load page fault) and the kernel kills only this payload. The
//! fault frame's stval carries the address it reached for.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"reaching into kernel memory...");
    // SAFETY(payload): deliberately illegal — reading a supervisor page from
    // U-mode raises a load page fault. That trap is the test.
    let _ = unsafe { core::ptr::read_volatile(0x8020_0000 as *const u8) };
    sys::write(b"if you see this, isolation failed");
}
