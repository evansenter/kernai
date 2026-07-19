/* Linker script for the kernai Doom payload (picolibc + doomgeneric).
 * Two page-aligned, W^X-clean PT_LOAD segments the kernel's ELF loader maps:
 *   text  R+X  — .text + .rodata + .got (constants live with code, never writable)
 *   data  R+W  — .data/.sdata, TLS template, .bss (incl. the 24 MiB heap), stack
 * Everything loads at its VMA (load == vaddr); the loader zero-fills .bss/stack.
 * The stack is an explicit reserved section so it falls inside the data segment's
 * memsz and therefore gets mapped (the loader maps only PT_LOAD pages). All the
 * __-symbols picolibc's crt0 needs (init_array, TLS, data/bss bounds, __stack)
 * are provided here. */

OUTPUT_ARCH(riscv)
ENTRY(_start)

MEMORY { mem (rwx) : ORIGIN = 0x10000, LENGTH = 0x08000000 }  /* 128 MiB VA window */

PHDRS {
    text PT_LOAD FLAGS(5);   /* R + X */
    data PT_LOAD FLAGS(6);   /* R + W */
    tls  PT_TLS;
}

SECTIONS {
    . = 0x10000;

    .text : {
        KEEP (*(.text.init.enter))
        KEEP (*(SORT_BY_NAME(.init) SORT_BY_NAME(.init.*)))
        *(.text.unlikely .text.unlikely.*)
        *(.text.startup .text.startup.*)
        *(.text .text.*)
        *(.gnu.linkonce.t.*)
        KEEP (*(.fini .fini.*))
        . = ALIGN(8);
        PROVIDE_HIDDEN (__preinit_array_start = .);
        KEEP (*(.preinit_array))
        PROVIDE_HIDDEN (__preinit_array_end = .);
        PROVIDE_HIDDEN (__init_array_start = .);
        KEEP (*(SORT_BY_INIT_PRIORITY(.init_array.*) SORT_BY_INIT_PRIORITY(.ctors.*)))
        KEEP (*(.init_array .ctors))
        PROVIDE_HIDDEN (__init_array_end = .);
        PROVIDE_HIDDEN (__fini_array_start = .);
        KEEP (*(SORT_BY_INIT_PRIORITY(.fini_array.*) SORT_BY_INIT_PRIORITY(.dtors.*)))
        KEEP (*(.fini_array .dtors))
        PROVIDE_HIDDEN (__fini_array_end = .);
    } :text

    .rodata : {
        *(.rdata)
        *(.rodata .rodata.*)
        *(.srodata.cst16) *(.srodata.cst8) *(.srodata.cst4) *(.srodata.cst2)
        *(.srodata .srodata.*)
        *(.gnu.linkonce.r.*)
    } :text
    .data.rel.ro : { *(.data.rel.ro .data.rel.ro.*) } :text
    .got : { *(.got.plt) *(.got) } :text

    /* Page-align the data segment onto its own pages: the kernel's ELF loader
     * maps a fresh frame per page and rejects a page mapped twice, so text
     * (R+X) and data (R+W) must not share the boundary page. A bare
     * `. = ALIGN(0x1000)` gets packed away here; the section-address form sticks. */
    .data : ALIGN(0x1000) {
        *(.data .data.*)
        *(.gnu.linkonce.d.*)
        . = ALIGN(8);
        PROVIDE( __global_pointer$ = . + 0x800 );
        *(.sdata .sdata.* .sdata2.*)
        *(.gnu.linkonce.s.*)
    } :data
    PROVIDE(__data_start = ADDR(.data));
    PROVIDE(__data_source = LOADADDR(.data));

    .tdata : {
        *(.tdata .tdata.* .gnu.linkonce.td.*)
        PROVIDE(__data_end = .);
        PROVIDE(__tdata_end = .);
    } :tls :data
    PROVIDE( __tls_base = ADDR(.tdata) );
    PROVIDE( __tdata_start = ADDR(.tdata) );
    PROVIDE( __tdata_source = LOADADDR(.tdata) );
    PROVIDE( __tdata_source_end = LOADADDR(.tdata) + SIZEOF(.tdata) );
    PROVIDE( __data_source_end = __tdata_source_end );
    PROVIDE( __tdata_size = SIZEOF(.tdata) );
    PROVIDE( __edata = __data_end );
    PROVIDE( _edata = __data_end );
    PROVIDE( __data_size = __data_end - __data_start );
    PROVIDE( __data_source_size = __data_source_end - __data_source );

    .tbss (NOLOAD) : {
        *(.tbss .tbss.* .gnu.linkonce.tb.*)
        *(.tcommon)
        PROVIDE( __tls_end = . );
        PROVIDE( __tbss_end = . );
    } :tls :data
    PROVIDE( __bss_start = ADDR(.tbss) );
    PROVIDE( __tbss_start = ADDR(.tbss) );
    PROVIDE( __tbss_offset = ADDR(.tbss) - ADDR(.tdata) );
    PROVIDE( __tbss_size = SIZEOF(.tbss) );
    PROVIDE( __tls_size = __tls_end - __tls_base );
    PROVIDE( __tls_align = MAX(ALIGNOF(.tdata), ALIGNOF(.tbss)) );
    PROVIDE( __tls_size_align = (__tls_size + __tls_align - 1) & ~(__tls_align - 1) );
    PROVIDE( __arm32_tls_tcb_offset = MAX(8, __tls_align) );
    PROVIDE( __arm64_tls_tcb_offset = MAX(16, __tls_align) );

    .bss (NOLOAD) : {
        *(.sbss .sbss.*)
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        PROVIDE( __bss_end = . );
    } :data
    PROVIDE( __bss_size = __bss_end - __bss_start );
    PROVIDE( __end = __bss_end );
    _end = __bss_end;
    PROVIDE( end = __bss_end );
    PROVIDE( __heap_start = __bss_end );

    .stack (NOLOAD) : ALIGN(16) {
        . += 1M;                 /* Doom's BSP recursion + flood fills want room */
        PROVIDE( __stack = . );
    } :data

    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment .note .note.* .riscv.attributes) }
}
