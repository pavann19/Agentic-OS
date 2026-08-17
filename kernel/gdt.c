#include "gdt.h"
#include "print.h"

GDTEntry gdt[3];
GDTDescriptor gdtr;

void gdt_set_entry(int index, uint32_t base, uint32_t limit, uint8_t access, uint8_t flags) {
    gdt[index].BaseLow = base & 0xFFFF;
    gdt[index].BaseMiddle = (base >> 16) & 0xFF;
    gdt[index].BaseHigh = (base >> 24) & 0xFF;
    gdt[index].LimitLow = limit & 0xFFFF;
    gdt[index].FlagsLimitHigh = ((limit >> 16) & 0x0F) | (flags & 0xF0);
    gdt[index].Access = access;
}

void gdt_init(void) {
    // Null descriptor
    gdt_set_entry(0, 0, 0, 0, 0);

    // Kernel Code Segment (Index 1, Offset 0x08)
    // Access: Present(1), Ring0(00), Executable(1), Dir/Conf(0), Readable(1), Accessed(0) -> 10011010b = 0x9A
    // Flags: Granularity(1), 32-bit(0), 64-bit(1), AVL(0) -> 1010b = 0xA0
    gdt_set_entry(1, 0, 0xFFFFF, 0x9A, 0xA0);

    // Kernel Data Segment (Index 2, Offset 0x10)
    // Access: Present(1), Ring0(00), Executable(0), Dir/Conf(0), Writable(1), Accessed(0) -> 10010010b = 0x92
    // Flags: Granularity(1), 32-bit(0), 64-bit(0), AVL(0) -> 1000b = 0x80
    gdt_set_entry(2, 0, 0xFFFFF, 0x92, 0x80);

    gdtr.Limit = sizeof(gdt) - 1;
    gdtr.Pointer = (uint64_t)&gdt;

    // Load GDT and refresh segments using inline assembly
    __asm__ __volatile__("lgdt %0" : : "m" (gdtr));
    __asm__ __volatile__(
        "push $0x08\n\t"
        "lea 1f(%%rip), %%rax\n\t"
        "push %%rax\n\t"
        "lretq\n\t"
        "1:\n\t"
        "mov $0x10, %%ax\n\t"
        "mov %%ax, %%ds\n\t"
        "mov %%ax, %%es\n\t"
        "mov %%ax, %%fs\n\t"
        "mov %%ax, %%gs\n\t"
        "mov %%ax, %%ss\n\t"
        : : : "rax"
    );

    printf("GDT Initialized.\n");
}
