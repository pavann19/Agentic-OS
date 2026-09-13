ENTRY(_start)

SECTIONS
{
    /* Distinct base from every other driver crate's own linker script --
       each ring-3 process gets its own fresh address space, this is
       purely for boot-log clarity when several run at once. */
    . = 0x00D00000;

    .text : { *(.text*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata*) }
    . = ALIGN(4096);
    .data : { *(.data*) }
    . = ALIGN(4096);
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) *(.got*) }
}
