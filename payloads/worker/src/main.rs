#![no_std]
#![no_main]
//! M10 delegation-chain leaf. Two greedy "request everything" hops upstream,
//! yet it is granted only {write} — the least privilege the chain's ceilings
//! allow. Writes and exits: proof that attenuation held end to end.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"worker: least privilege after a two-hop delegation chain");
}
