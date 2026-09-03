ENTRY(_start)

SECTIONS
{
    /* Fixed, arbitrary base -- kernel_rs/src/elf.rs maps segments at
       their real p_vaddr from the program headers, so this base just
       needs to not collide with the fixed 0x600000/0x700000 the
       hand-built-blob demos (init.rs, the Phase 1/2 demo_ring3 proof)
       use -- and it wouldn't even if it did, since every ring-3 process
       here gets its own fresh address space (vmm::new_address_space).
       Kept distinct anyway for clarity when reading a boot log. */
    . = 0x00500000;

    /* Page-align each output section onto its own PT_LOAD segment. Real
       bug found bringing this crate up: without this, .text (RX) and
       .rodata (R, non-executable) packed into the SAME 4KB page since
       the whole binary is tiny -- elf.rs's loader maps PT_LOAD segments
       page by page, so the second segment's non-executable mapping for
       that shared page silently clobbered the first segment's
       executable mapping, producing an instruction-fetch #PF (vector
       14, error_code=0x15) at the entry point itself. Real ELF loaders
       rely on PT_LOAD segments being page-aligned in the FILE for
       exactly this reason (p_vaddr === p_offset mod p_align, per spec);
       the custom linker script here just wasn't enforcing it. */
    .text : { *(.text*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata*) }
    . = ALIGN(4096);
    .data : { *(.data*) }
    . = ALIGN(4096);
    .bss : { *(.bss*) }

    /DISCARD/ : { *(.comment) *(.eh_frame) *(.note*) *(.got*) }
}
