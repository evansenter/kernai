//! Hardware abstraction layer — the *only* unsafe island in the kernel
//! (CLAUDE.md hard rules: ≤4 files with `unsafe`, ≤200 lines inside unsafe,
//! enforced by ci/unsafe_budget.sh). Everything above this module is
//! `#![forbid(unsafe_code)]`.
//!
//! Keep exports minimal and safe: the contract is "callers cannot cause
//! memory unsafety through this API".

mod boot;
mod sbi;

pub use sbi::{console_putchar, shutdown};
