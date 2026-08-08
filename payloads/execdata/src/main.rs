#![no_std]
#![no_main]
//! E1 stimulus: jump into writable data. The target page is mapped and
//! readable — but not executable, so the fetch faults (scause 12) exactly
//! like `badjump`... except the page-table walk tells a different story:
//! badjump fetches from NOTHING (v=0), this fetches from a live data page
//! (v=1, x=0) — the W^X denial on the fetch side. Only the walk separates
//! the two diagnoses; the classic line renders them identically.

static mut DATA: [u32; 4] = [0x0000_0013; 4]; // `nop` encodings, in .bss (RW)

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"jumping into a data page");
    // SAFETY: deliberately faulting — fetching from a non-executable page
    // raises an instruction page fault; that trap is the test.
    unsafe {
        core::arch::asm!("jalr {0}", in(reg) (&raw const DATA).cast::<u8>());
    }
}
