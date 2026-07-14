/* Payloads link at a low virtual address (M5): each payload gets its own
 * Sv39 address space, so the VA is private and never collides with the
 * kernel (which lives high, at 0x8000_0000+, as a supervisor gigapage). The
 * kernel's ELF loader maps these segments into the payload's page table.
 *
 * Explicit PHDRS force three page-aligned PT_LOAD segments with correct,
 * W^X-clean permissions:
 *   text   R+X  — code (never writable)
 *   rodata R    — constants
 *   data   R+W  — .data, .bss, and the stack (never executable)
 * The loader maps each segment's pages with those permissions; because each
 * segment starts on its own page, no page is mapped twice and W^X holds. */

OUTPUT_ARCH(riscv)
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5); /* PF_R | PF_X */
    rodata PT_LOAD FLAGS(4); /* PF_R */
    data   PT_LOAD FLAGS(6); /* PF_R | PF_W */
}

SECTIONS
{
    . = 0x10000;

    .text : {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :text

    /* Bare `. = ALIGN` statements force a fresh page between segments even
     * when a section is empty (an empty output section's own ALIGN is
     * dropped, which previously let two segments share a page). */
    . = ALIGN(0x1000);
    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } :rodata

    . = ALIGN(0x1000);
    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    } :data

    .bss (NOLOAD) : {
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    } :data

    /* User stack, in the writable data segment; _start sets sp here. */
    .stack (NOLOAD) : ALIGN(16) {
        . += 16K;
        __stack_top = .;
    } :data

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
