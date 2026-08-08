#![no_std]
#![no_main]
//! E7 stimulus: fuzz the attenuation lattice (P10). Holding {write,yield,spawn},
//! spawn workers with a sweep of requested-cap masks — including masks far
//! wider than the worker's {write} ceiling and the empty mask. The kernel
//! emits a `payload_spawn` event per attempt with parent/requested/granted;
//! the e7 check asserts `granted == requested & parent & ceiling` (and so
//! `granted ⊆ parent`) for every single one. Any widening is a lattice bug.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"fuzzing the attenuation lattice");
    for req in [7usize, 5, 3, 1, 0] {
        let _ = sys::spawn(sys::SPAWNABLE_WORKER, req);
    }
}
