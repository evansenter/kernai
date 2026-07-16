    .section .text.entry
    .globl _start
_start:
    la   sp, __stack_top
    call pmain
    li   a0, 0
    li   a7, 0          // SYS_EXIT on return
    ecall
1:  j    1b
