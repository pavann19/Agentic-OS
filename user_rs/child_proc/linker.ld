ENTRY(_start)

SECTIONS
{
    . = 0x00A00000;

    .text : { *(.text*) }
    . = ALIGN(4096);
    .rodata : {
        *(.rodata*)
        *(.got*)
        *(.data.rel.ro*)
    }
    . = ALIGN(4096);
    .data : { *(.data*) }
    . = ALIGN(4096);
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) }
}
