.section .text.r5s_enter, "ax"
.global r5s_enter
.type r5s_enter, %function
r5s_enter:
    adrp x9, r5s_return_sp
    add x9, x9, :lo12:r5s_return_sp
    mov x10, sp
    str x10, [x9]
    dsb sy
    adr x30, r5s_return_trampoline
    br x0
r5s_return_trampoline:
    adrp x9, r5s_return_sp
    ldr x9, [x9, :lo12:r5s_return_sp]
    mov sp, x9
    b r5s_returned
.section .bss.r5s_return_sp, "aw", %nobits
.balign 8
r5s_return_sp: .skip 8
