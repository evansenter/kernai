/* Acceptance payloads link at the kernel's payload arena (0x8040_0000,
 * 2 MiB). No paging until M5, so virtual == physical and the address is a
 * contract with kernel/src/payload.rs. The kernel asserts at boot that its
 * own image ends below this. */

OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE = 0x80400000;

SECTIONS
{
    . = BASE;

    .text : {
        KEEP(*(.text.entry))
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
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }

    /* Payload stack: the kernel's ELF loader zero-fills all NOLOAD space
     * (memsz beyond filesz), and _start points sp here. 16 KiB. */
    .stack (NOLOAD) : ALIGN(16) {
        . += 16K;
        __stack_top = .;
    }

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
