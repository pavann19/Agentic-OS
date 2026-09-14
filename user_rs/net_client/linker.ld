ENTRY(_start)

SECTIONS
{
    . = 0x00E00000;

    .text : { *(.text*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata*) }
    . = ALIGN(4096);
    .data : { *(.data*) }
    . = ALIGN(4096);
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) *(.got*) }
}
