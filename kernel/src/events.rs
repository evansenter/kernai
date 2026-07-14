#![forbid(unsafe_code)]
//! Kernel event identity. Every frame the kernel emits is an event with a
//! globally monotonic `id` — the spine that P12's causal graph will hang
//! parent references off later (each event will gain a `cause` id).

use core::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Allocate the next event id. Single hart, so Relaxed is enough; ids are
/// unique and monotonic by construction.
pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}
