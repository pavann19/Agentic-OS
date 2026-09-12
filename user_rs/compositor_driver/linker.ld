ENTRY(_start)

SECTIONS
{
    /* Distinct base from every other driver crate's own linker script --
       each ring-3 process gets its own fresh address space, this is
       purely for boot-log clarity when several run at once. */
    . = 0x00900000;

    /* Real bug found bringing up user_rs/serial_driver, applies to every
       driver crate in this repo since: page-align every output section
       onto its own PT_LOAD segment, or a small binary packs mixed
       permissions into one shared page and elf.rs's page-by-page
       PT_LOAD mapping silently clobbers whichever segment applied first. */
    .text : { *(.text*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata*) }
    . = ALIGN(4096);
    .data : { *(.data*) }
    . = ALIGN(4096);
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) *(.got*) }
}
