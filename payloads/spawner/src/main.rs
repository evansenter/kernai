#![no_std]
#![no_main]
//! M4 delegation fixture. Holds {write, spawn} but NOT yield.
//!  1. Calls yield → must be refused (ENOCAP): the cap gate applies to us.
//!  2. Spawns a child requesting {write, yield}; the kernel must attenuate
//!     the grant to {write} — a child can never hold a capability its parent
//!     lacks (P10). The spawn event shows requested vs granted.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    let _ = sys::yield_now(); // we lack CAP_YIELD → ENOCAP
    sys::spawn(sys::SPAWNABLE_CHILD, sys::CAP_WRITE | sys::CAP_YIELD);
}
