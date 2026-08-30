ENTRY(_start)

SECTIONS
{
    /* Distinct base from serial_driver's 0x500000 purely for boot-log
       clarity -- each ring-3 process gets its own fresh address space
       (vmm::new_address_space), so there's no real collision risk
       either way. */
    . = 0x00520000;

    /* Real bug found bringing up user_rs/serial_driver applies here too:
       page-align every output section onto its own PT_LOAD segment, or
       a small binary packs RX/R(/RW) sections into one shared page and
       elf.rs's page-by-page PT_LOAD mapping silently clobbers whichever
       segment's permissions were applied first. */
    .text : { *(.text*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata*) }
    . = ALIGN(4096);
    .data : { *(.data*) }
    . = ALIGN(4096);
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) *(.got*) }
}
