#![no_std]
#![no_main]
//! M10 delegation-chain root (hop 1 of 2). Holds {write, spawn, yield}. It
//! spawns a redelegator, *greedily* requesting every capability (`!0`); the
//! kernel must attenuate the grant to what this payload itself holds ∩ the
//! child image's ceiling (P10) — a delegator can never mint authority it lacks.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"delegator: spawning a redelegator, requesting all caps");
    sys::spawn(sys::SPAWNABLE_REDELEGATOR, !0);
}
