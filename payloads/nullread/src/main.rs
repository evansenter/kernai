#![no_std]
#![no_main]
//! E1 stimulus: the classic null-pointer dereference. VA 0 is never mapped in
//! a payload address space, so the load faults (scause 13) with stval = 0 —
//! same cause class as `wild` (which reaches into KERNEL memory), but a
//! different root cause the page-table walk separates: wild hits a mapped
//! supervisor page (v=1, u=0); this hits nothing at all (v=0 at VA 0).

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"dereferencing null");
    // SAFETY: deliberately faulting — VA 0 is unmapped; the load page fault
    // with stval=0 is the test.
    let _ = unsafe { core::ptr::read_volatile(core::ptr::null::<u8>()) };
    sys::write(b"if you see this, null was mapped");
}
