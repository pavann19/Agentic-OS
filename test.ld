ENTRY(kernel_entry_phys) KERNEL_VMA = 0xFFFFFFFF80000000; kernel_entry_phys = kernel_entry - KERNEL_VMA; SECTIONS { . = KERNEL_VMA + 0x200000; .text : AT(ADDR(.text) - KERNEL_VMA) { *(.text*) } }
