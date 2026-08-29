/* Higher-half kernel. VMA (where code runs, what it's linked against) and
   LMA (where the bootloader physically places the bytes) differ by a
   constant KERNEL_VIRTUAL_BASE offset via AT() — this is what makes a
   higher-half kernel possible: the ELF's p_vaddr and p_paddr differ per
   PT_LOAD segment, and boot_rs's bootstrap page tables map that offset
   before jumping to the (now valid) higher-half entry point.

   KERNEL_VIRTUAL_BASE and KERNEL_PHYSICAL_BASE MUST match
   kernel_rs/src/vmm.rs's KERNEL_VIRTUAL_BASE and boot_rs's bootstrap
   mapping — three independent places agreeing on one constant, same
   discipline as the BootInfo ABI mirror.

   Boundary-symbol placement (`. = ALIGN(0x1000); __x_start = .;` BEFORE
   each section, not `__x_start = .;` immediately followed by the
   section's own ALIGN) is deliberate, not stylistic — a real bug this
   session hit and fixed: a symbol assigned right before a section that
   has its own ALIGN() captures the PRE-alignment location counter, not
   the section's true (aligned) start. That made adjacent segments'
   vmm::init()-computed page ranges overlap by one page, and the LATER
   segment's more-restrictive permissions (e.g. rodata's NX) silently
   overwrote the EARLIER segment's needed ones (text's executable bit) for
   that shared page — confirmed by reading back the actual PTE and finding
   NX set on a page containing live, executing kernel code. Every
   start/end symbol below is placed at an EXPLICIT, already-aligned
   location counter value for exactly this reason. */

KERNEL_VIRTUAL_BASE = 0xFFFFFFFF80000000;
KERNEL_PHYSICAL_BASE = 0x200000;

ENTRY(kernel_main)

SECTIONS {
    . = KERNEL_VIRTUAL_BASE + KERNEL_PHYSICAL_BASE;

    __kernel_vaddr_start = .;
    __kernel_paddr_start = . - KERNEL_VIRTUAL_BASE;

    . = ALIGN(0x1000);
    __text_start = .;
    .text : AT(ADDR(.text) - KERNEL_VIRTUAL_BASE) {
        *(.text*)
    }
    __text_end = .;

    . = ALIGN(0x1000);
    __rodata_start = .;
    .rodata : AT(ADDR(.rodata) - KERNEL_VIRTUAL_BASE) {
        *(.rodata*)
        /* .got/.data.rel.ro* are LLD orphan sections for relocated-pointer
           data that's read-only after startup -- claimed here explicitly
           rather than left to default orphan placement (which put .got at
           an unaligned physical address in its own separate PT_LOAD
           segment the first time this was built without claiming it).
           Read-only is the semantically correct home for both anyway. */
        *(.got*)
        *(.data.rel.ro*)
    }
    __rodata_end = .;

    . = ALIGN(0x1000);
    __data_start = .;
    .data : AT(ADDR(.data) - KERNEL_VIRTUAL_BASE) {
        *(.data*)
    }
    __data_end = .;

    . = ALIGN(0x1000);
    __bss_start = .;
    .bss : AT(ADDR(.bss) - KERNEL_VIRTUAL_BASE) {
        *(COMMON)
        *(.bss*)
    }
    __bss_end = .;

    . = ALIGN(0x1000);
    __kernel_vaddr_end = .;
    __kernel_paddr_end = . - KERNEL_VIRTUAL_BASE;

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note*)
    }
}
