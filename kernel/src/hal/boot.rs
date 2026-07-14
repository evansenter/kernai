//! Entry point: OpenSBI jumps here at 0x8020_0000 in S-mode with
//! a0 = hartid, a1 = device-tree pointer (single hart; SMP is a non-goal).

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text.entry
    .globl _start
_start:
    // Rust has no stack yet: set one up, zero .bss, then never return here.
    la   sp, __stack_top

    la   t0, __bss_start
    la   t1, __bss_end          // both 16-aligned by the linker script
1:  bgeu t0, t1, 2f
    sd   zero, 0(t0)
    addi t0, t0, 8
    j    1b

2:  call __kernai_start
3:  wfi                         // unreachable: __kernai_start diverges
    j    3b
    "#
);

// SAFETY: `__kernai_start` is the sole entry from the boot asm above; it runs
// exactly once, on one hart, after sp and .bss are valid. The unmangled name
// is required so the asm can name it. It must diverge — there is no caller
// frame to return to.
#[unsafe(no_mangle)]
extern "C" fn __kernai_start(hartid: usize, dtb: usize) -> ! {
    crate::kmain(hartid, dtb)
}

// SAFETY: these symbols are defined by link.ld; only their *addresses* are
// ever taken (via &raw const), never their contents read through these
// declarations.
unsafe extern "C" {
    static __kernel_end: u8;
    static __stack_top: u8;
    static __trap_stack_top: u8;
}

/// First address past everything the kernel image occupies.
pub fn kernel_end() -> usize {
    (&raw const __kernel_end) as usize
}

/// Top of the boot stack (re-set when execution re-enters the idle loop —
/// nothing on the boot stack outlives a payload switch).
pub fn boot_stack_top() -> usize {
    (&raw const __stack_top) as usize
}

/// Top of the dedicated stack that traps-from-U-mode land on.
pub fn trap_stack_top() -> usize {
    (&raw const __trap_stack_top) as usize
}

/// Payload arena: fixed physical range where payload ELFs are loaded and
/// run (no paging until M5, so this address is baked into payloads/link.ld).
pub const ARENA_BASE: usize = 0x8040_0000;
pub const ARENA_SIZE: usize = 2 * 1024 * 1024;

fn arena_slice(offset: usize, len: usize) -> Option<*mut u8> {
    let end = offset.checked_add(len)?;
    if end > ARENA_SIZE {
        return None;
    }
    Some((ARENA_BASE + offset) as *mut u8)
}

/// Copy `bytes` into the arena at `offset`. False if out of bounds.
/// Bounds-checked raw memory access so the ELF loader stays safe code.
pub fn arena_write(offset: usize, bytes: &[u8]) -> bool {
    match arena_slice(offset, bytes.len()) {
        // SAFETY: dst lies wholly inside the arena, which link.ld/boot keep
        // disjoint from every kernel section (asserted at boot via
        // kernel_end()); `bytes` is a kernel slice, so src/dst can't overlap.
        Some(dst) => unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
            true
        },
        None => false,
    }
}

/// Zero `len` bytes of the arena at `offset`. False if out of bounds.
pub fn arena_zero(offset: usize, len: usize) -> bool {
    match arena_slice(offset, len) {
        // SAFETY: range is wholly inside the arena (see arena_write).
        Some(dst) => unsafe {
            core::ptr::write_bytes(dst, 0, len);
            true
        },
        None => false,
    }
}

/// Copy arena bytes at `offset` into `buf` (payload memory → kernel buffer,
/// e.g. for the write syscall). False if out of bounds.
pub fn arena_read(offset: usize, buf: &mut [u8]) -> bool {
    match arena_slice(offset, buf.len()) {
        // SAFETY: src is wholly inside the arena; dst is a kernel slice.
        Some(src) => unsafe {
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len());
            true
        },
        None => false,
    }
}
