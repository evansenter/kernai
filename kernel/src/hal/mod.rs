//! Hardware abstraction layer — the *only* unsafe island in the kernel
//! (CLAUDE.md hard rules: ≤4 files with `unsafe`, ≤200 lines inside unsafe,
//! enforced by ci/unsafe_budget.sh). Everything above this module is
//! `#![forbid(unsafe_code)]`.
//!
//! Keep exports minimal and safe: the contract is "callers cannot cause
//! memory unsafety through this API".

mod boot;
mod csr;
mod sbi;
mod trap;

pub use boot::{
    POOL_BASE, POOL_END, boot_stack_top, kernel_end, phys_read, phys_read_u64, phys_write,
    phys_write_u64, phys_zero,
};
pub use csr::{
    enable_timer_interrupts, read_scause, read_stval, read_time, wait_for_interrupt,
    without_interrupts, write_satp,
};
pub use sbi::{console_getchar, console_putchar, set_timer, shutdown};
pub use trap::{enter_user, init as traps_init, trigger_illegal_instruction};
