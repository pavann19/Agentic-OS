ENTRY(_start)

SECTIONS
{
    . = 0x00A00000;

    .text : { *(.text*) }
    .rodata : {
        *(.rodata*)
        *(.got*)
        *(.data.rel.ro*)
    }
    .data : { *(.data*) }
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) }
}
