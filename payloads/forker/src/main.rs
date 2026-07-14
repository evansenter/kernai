#![no_std]
#![no_main]
//! M6 checkpoint fixture (P8). Prints once *before* the snapshot, then calls
//! sys::snapshot(). The kernel later restores/forks that checkpoint into new
//! payloads: each resumes right here, at the return of snapshot(), NOT from
//! the top — so "before the checkpoint" is printed exactly once (by the
//! original) while the post-snapshot code runs in every continuation.
//!
//! snapshot() returns a positive id in the original and 0 in a restored
//! continuation — the fork()-style branch that makes this a what-if verb.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"before the checkpoint");
    let r = sys::snapshot();
    if r > 0 {
        sys::write(b"original branch (kept running)");
    } else {
        sys::write(b"restored continuation (resumed from checkpoint)");
    }
    sys::write(b"common tail after the checkpoint");
}
