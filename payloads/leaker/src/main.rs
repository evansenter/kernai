#![no_std]
#![no_main]
//! M5 confused-deputy fixture. A payload can't read kernel memory directly
//! (the MMU faults — see `wild`). This tries the *indirect* path: hand the
//! kernel a pointer into its own memory via write(), hoping the kernel reads
//! it on the payload's behalf. The kernel must refuse (EFAULT) because the
//! pointer isn't user-accessible — so no bytes leak. Exits 9 if the write
//! was (correctly) refused, 0 if it leaked (a bug).

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    // 0x80200000 is kernel memory (supervisor-only in our page table).
    let r = sys::write_raw(0x8020_0000, 32);
    sys::exit(if r < 0 { 9 } else { 0 });
}
