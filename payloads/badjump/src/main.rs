#![no_std]
#![no_main]
//! E1 stimulus (M12): an instruction-fetch fault, distinct from the data-side
//! faults (crasher = illegal instruction, wild = load, wxviol = store). Jumps
//! to address 0 — unmapped in the payload's address space — so the CPU faults
//! on the *instruction fetch* (scause 12, instruction_page_fault), sepc = 0.
//! The agentic frame's pagewalk shows the level-2 entry is not present (v=0):
//! "no mapping", a different root cause than wild's "mapped but supervisor".

// SAFETY: called once by sys's `_start`; unmangled so the asm can name it.
#[unsafe(no_mangle)]
extern "C" fn pmain() {
    sys::write(b"badjump: jumping to an unmapped instruction address");
    // SAFETY: deliberately faults — a jump to VA 0 fetches an unmapped
    // instruction, raising instruction_page_fault. Never returns.
    unsafe { core::arch::asm!("jr {0}", in(reg) 0usize, options(noreturn)) }
}
