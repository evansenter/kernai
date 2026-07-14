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

/// Physical frame pool: the RAM the kernel hands out as 4 KiB frames (for
/// page tables and per-payload memory, M5). Starts 2 MiB above the kernel
/// load base — well clear of the ~150 KiB kernel image — and runs to the end
/// of qemu-virt's 128 MiB DRAM. The kernel identity-maps this range as a
/// supervisor gigapage, so a physical address in the pool is also its kernel
/// virtual address; `phys_*` access it directly.
pub const POOL_BASE: usize = 0x8040_0000;
pub const POOL_END: usize = 0x8800_0000; // 0x8000_0000 + 128 MiB

fn pool_ptr(pa: usize, len: usize) -> Option<*mut u8> {
    let end = pa.checked_add(len)?;
    if pa < POOL_BASE || end > POOL_END {
        return None;
    }
    Some(pa as *mut u8)
}

/// Copy `bytes` to physical address `pa` (must lie in the pool). False if not.
/// Bounds-checked raw access so the frame allocator / loader stay safe code.
pub fn phys_write(pa: usize, bytes: &[u8]) -> bool {
    match pool_ptr(pa, bytes.len()) {
        // SAFETY: dst lies wholly in the pool (disjoint from the kernel image,
        // asserted at boot); `bytes` is a kernel slice so src/dst can't overlap.
        Some(dst) => unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
            true
        },
        None => false,
    }
}

/// Zero `len` bytes at physical `pa`. False if out of the pool.
pub fn phys_zero(pa: usize, len: usize) -> bool {
    match pool_ptr(pa, len) {
        // SAFETY: range is wholly in the pool (see phys_write).
        Some(dst) => unsafe {
            core::ptr::write_bytes(dst, 0, len);
            true
        },
        None => false,
    }
}

/// Copy `buf.len()` bytes from physical `pa` into `buf`. False if out of pool.
pub fn phys_read(pa: usize, buf: &mut [u8]) -> bool {
    match pool_ptr(pa, buf.len()) {
        // SAFETY: src is wholly in the pool; dst is a kernel slice.
        Some(src) => unsafe {
            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len());
            true
        },
        None => false,
    }
}

/// Read a naturally-aligned u64 (a page-table entry) at physical `pa`.
pub fn phys_read_u64(pa: usize) -> Option<u64> {
    if !pa.is_multiple_of(8) {
        return None;
    }
    let ptr = pool_ptr(pa, 8)?;
    // SAFETY: ptr is in the pool and 8-aligned; PTEs are the only 8-byte
    // physical reads and always land on aligned slots.
    Some(unsafe { core::ptr::read_volatile(ptr as *const u64) })
}

/// Write a naturally-aligned u64 (a page-table entry) at physical `pa`.
pub fn phys_write_u64(pa: usize, val: u64) -> bool {
    if !pa.is_multiple_of(8) {
        return false;
    }
    match pool_ptr(pa, 8) {
        // SAFETY: ptr is in the pool and 8-aligned (see phys_read_u64).
        Some(ptr) => unsafe {
            core::ptr::write_volatile(ptr as *mut u64, val);
            true
        },
        None => false,
    }
}
