/* Same load model as the current C kernel (kernel/linker.ld): low-half,
   loaded at 2MB by the exact-physical-address ELF loader in boot/main.c.
   Phase 0's higher-half VMM redesign will replace this file, not patch it —
   tracked in PHASE0_PROGRESS.md. */

ENTRY(kernel_main)

SECTIONS {
    . = 0x200000;

    .text : ALIGN(0x1000) {
        *(.text*)
    }

    .rodata : ALIGN(0x1000) {
        *(.rodata*)
    }

    .data : ALIGN(0x1000) {
        *(.data*)
    }

    .bss : ALIGN(0x1000) {
        *(COMMON)
        *(.bss*)
    }

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note*)
    }
}
