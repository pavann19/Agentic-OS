ENTRY(_start)

SECTIONS
{
    . = 0x00560000;

    /* Same page-alignment fix every driver crate in this repo needed
       (see serial_driver/linker.ld's comment for the original bug):
       page-align every output section onto its own PT_LOAD segment. */
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
