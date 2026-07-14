#![no_std]
#![no_main]
//! M4 liveness fixture (P1/P2 seed): announces itself, then spins forever.
//! The kernel's instruction-count deadline must preempt it and emit a
//! structured `payload_killed` event — a runaway payload is a parked event,
//! not a hung machine. The operator never had to intervene.

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"looping forever");
    loop {
        core::hint::spin_loop();
    }
}
