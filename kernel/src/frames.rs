#![forbid(unsafe_code)]
//! Physical frame allocator: hands out 4 KiB frames from the pool
//! (`hal::POOL_BASE..hal::POOL_END`) for page tables and payload memory.
//!
//! A plain bitmap (P11: serializable, no hidden state) protected by the same
//! structural discipline as the rest of the kernel — single hart, mutated
//! only from the scheduler / trap handler, never concurrently. Freed frames
//! are returned to the pool so many payload runs don't exhaust it.

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const RE: Ordering = Ordering::Relaxed;

pub const FRAME_SIZE: usize = 4096;

const POOL_FRAMES: usize = (hal::POOL_END - hal::POOL_BASE) / FRAME_SIZE;
const BITMAP_WORDS: usize = POOL_FRAMES.div_ceil(64);

use crate::hal;

/// One bit per frame: 1 = allocated. AtomicU64 words give interior mutability
/// in a static without `unsafe`; consistency is structural, not atomic.
static BITMAP: [AtomicU64; BITMAP_WORDS] = [const { AtomicU64::new(0) }; BITMAP_WORDS];
/// Hint for the next search; purely an optimization.
static NEXT_HINT: AtomicUsize = AtomicUsize::new(0);
/// Live allocation count (for the /memory/framemap resource, P11).
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

fn test(frame: usize) -> bool {
    BITMAP[frame / 64].load(RE) & (1 << (frame % 64)) != 0
}
fn set(frame: usize) {
    let w = &BITMAP[frame / 64];
    w.store(w.load(RE) | (1 << (frame % 64)), RE);
}
fn clear(frame: usize) {
    let w = &BITMAP[frame / 64];
    w.store(w.load(RE) & !(1 << (frame % 64)), RE);
}

/// Allocate one zeroed frame; returns its physical address, or None if the
/// pool is exhausted. The frame is zeroed so fresh page tables start empty
/// and payload `.bss` is clean.
pub fn alloc() -> Option<usize> {
    let start = NEXT_HINT.load(RE) % POOL_FRAMES;
    for i in 0..POOL_FRAMES {
        let frame = (start + i) % POOL_FRAMES;
        if !test(frame) {
            set(frame);
            ALLOCATED.fetch_add(1, RE);
            NEXT_HINT.store(frame + 1, RE);
            let pa = hal::POOL_BASE + frame * FRAME_SIZE;
            hal::phys_zero(pa, FRAME_SIZE);
            return Some(pa);
        }
    }
    None
}

/// Return a frame to the pool. `pa` must be a frame this allocator handed out.
pub fn free(pa: usize) {
    if !(hal::POOL_BASE..hal::POOL_END).contains(&pa)
        || !(pa - hal::POOL_BASE).is_multiple_of(FRAME_SIZE)
    {
        return;
    }
    let frame = (pa - hal::POOL_BASE) / FRAME_SIZE;
    if test(frame) {
        clear(frame);
        ALLOCATED.fetch_sub(1, RE);
    }
}

/// Live frame count and pool capacity — the `memory` MCP resource (P11), and
/// E7's allocator-soundness probe (allocated must return to baseline after
/// every suite: no leak across the reap paths).
pub fn stats() -> (usize, usize) {
    (ALLOCATED.load(RE), POOL_FRAMES)
}
