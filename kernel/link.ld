/* qemu -machine virt: DRAM starts at 0x8000_0000; OpenSBI (fw_dynamic)
 * occupies the bottom and jumps to 0x8020_0000 in S-mode. Link there. */

OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE = 0x80200000;

SECTIONS
{
    . = BASE;

    .text : {
        KEEP(*(.text.entry))    /* _start must sit exactly at BASE */
        *(.text .text.*)
    }

    .rodata : ALIGN(16) {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    .data : ALIGN(16) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot stack for the single hart: 64 KiB, grows down from __stack_top.
     * Outside __bss_start..__bss_end on purpose: stacks need no zeroing. */
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 64K;
        __stack_top = .;
    }

    /* Kernel trap stack: traps arriving from U-mode land here (sscratch
     * points at __trap_stack_top while a payload runs). 16 KiB. */
    .trapstack (NOLOAD) : ALIGN(16) {
        . += 16K;
        __trap_stack_top = .;
    }

    /* End of everything the kernel image occupies. Boot asserts this stays
     * below the payload arena at 0x8040_0000 (see src/payload.rs). */
    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
