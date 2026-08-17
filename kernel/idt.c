#include "idt.h"
#include "print.h"

IDTEntry idt[256];
IDTDescriptor idtr;

void idt_set_descriptor(uint8_t vector, void* isr, uint8_t flags) {
    uint64_t descriptor = (uint64_t)isr;

    idt[vector].OffsetLow = descriptor & 0xFFFF;
    idt[vector].Selector = 0x08; // Kernel Code Segment from our GDT
    idt[vector].IST = 0;
    idt[vector].Type_Attr = flags;
    idt[vector].OffsetMiddle = (descriptor >> 16) & 0xFFFF;
    idt[vector].OffsetHigh = (descriptor >> 32) & 0xFFFFFFFF;
    idt[vector].Zero = 0;
}

void idt_init(void) {
    idtr.Limit = sizeof(idt) - 1;
    idtr.Pointer = (uint64_t)&idt;

    // Load IDT
    __asm__ __volatile__("lidt %0" : : "m" (idtr));

    printf("IDT Initialized.\n");
}
