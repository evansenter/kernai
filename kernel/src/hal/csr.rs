//! Raw CSR access. Every wrapper is safe to call: none of them can be used
//! to violate memory safety from outside hal (stvec installation, the one
//! footgun, is crate-internal and only ever fed hal's own trap vector).

use core::arch::asm;

const SSTATUS_SIE: u64 = 1 << 1; // supervisor interrupt enable
const SIE_STIE: u64 = 1 << 5; // supervisor timer interrupt enable

pub fn read_scause() -> u64 {
    let v: u64;
    // SAFETY: csrr from scause reads a CSR; no memory access, no side effects.
    unsafe { asm!("csrr {}, scause", out(reg) v, options(nomem, nostack)) };
    v
}

pub fn read_stval() -> u64 {
    let v: u64;
    // SAFETY: csrr from stval reads a CSR; no memory access, no side effects.
    unsafe { asm!("csrr {}, stval", out(reg) v, options(nomem, nostack)) };
    v
}

/// Current timebase value (10 MHz on qemu-virt; icount-derived, P9).
pub fn read_time() -> u64 {
    let v: u64;
    // SAFETY: rdtime reads the time CSR; no memory access, no side effects.
    unsafe { asm!("rdtime {}", out(reg) v, options(nomem, nostack)) };
    v
}

/// Install the supervisor trap vector (direct mode). Crate-internal: only
/// hal::trap::init calls this, with hal's own 16-aligned vector.
pub(super) fn write_stvec(addr: usize) {
    // SAFETY: addr is __trap_vector (asm in hal::trap), 16-aligned so bits
    // [1:0] are 00 = direct mode. All traps from here on land there.
    unsafe { asm!("csrw stvec, {}", in(reg) addr, options(nomem, nostack)) };
}

/// Set sscratch: the trap vector's "where is the kernel stack" register.
/// Convention: while in U-mode it holds the kernel trap-stack top; while in
/// S-mode it holds 0. Crate-internal — only hal::trap manages it.
pub(super) fn write_sscratch(value: usize) {
    // SAFETY: sscratch is a scratch CSR with no side effects; the trap
    // vector's swap logic is the only consumer. No memory access.
    unsafe { asm!("csrw sscratch, {}", in(reg) value, options(nomem, nostack)) };
}

/// Enable supervisor timer interrupts (sie.STIE, then sstatus.SIE).
/// Call only after the trap vector is installed.
pub fn enable_timer_interrupts() {
    // SAFETY: sets two enable bits; the only interrupt this can unmask is
    // the timer, which hal::trap's installed vector services. No memory.
    // `nostack` only — this must also act as a compiler barrier.
    unsafe {
        asm!("csrs sie, {}", in(reg) SIE_STIE, options(nostack));
        asm!("csrs sstatus, {}", in(reg) SSTATUS_SIE, options(nostack));
    }
}

/// Run `f` with interrupts disabled, restoring the previous state after.
/// This is the kernel's only critical-section primitive (single hart): frame
/// emission and ring reads use it so a timer trap can never interleave.
pub fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let prev: u64;
    // SAFETY: atomically clears sstatus.SIE and returns the old value; acts
    // as a compiler barrier (no `nomem`) so memory ops can't be hoisted out
    // of the critical section.
    unsafe { asm!("csrrc {}, sstatus, {}", out(reg) prev, in(reg) SSTATUS_SIE, options(nostack)) };
    let out = f();
    if prev & SSTATUS_SIE != 0 {
        // SAFETY: re-sets sstatus.SIE previously cleared above; barrier as above.
        unsafe { asm!("csrs sstatus, {}", in(reg) SSTATUS_SIE, options(nostack)) };
    }
    out
}

/// Park until an interrupt is pending (the idle loop's heartbeat).
pub fn wait_for_interrupt() {
    // SAFETY: wfi stalls the hart until an interrupt pends, then execution
    // continues normally. The trap handler that runs during the park mutates
    // kernel state, so no `nomem` — like the other interrupt asm here, this
    // must be a compiler barrier so those effects are visible to the caller.
    unsafe { asm!("wfi", options(nostack)) };
}
