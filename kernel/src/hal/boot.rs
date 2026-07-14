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
