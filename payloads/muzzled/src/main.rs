#![no_std]
#![no_main]
//! M4 capability fixture: a payload with an empty CapSet. Its `write` must
//! be refused with ENOCAP; it exits 7 so the operator can see it noticed
//! the denial (the kernel also emits a structured `syscall_denied` event).

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    let r = sys::write(b"i should not be able to say this");
    sys::exit(if r < 0 { 7 } else { 0 });
}
