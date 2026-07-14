#![no_std]
#![no_main]
//! M10 delegation-chain middle (hop 2 of 2). Its parent requested everything
//! for it, but the redelegator image ceiling is {write, spawn} — so it never
//! held `yield`. It re-delegates, again greedily requesting all caps for a
//! worker; the grant attenuates again to the worker's ceiling ({write}). The
//! chain can only ever shrink, never re-widen at a hop.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"redelegator: re-delegating to a worker, requesting all caps");
    sys::spawn(sys::SPAWNABLE_WORKER, !0);
}
