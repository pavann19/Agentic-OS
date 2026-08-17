#pragma once
#include <stdint.h>

typedef struct {
    uint16_t OffsetLow;
    uint16_t Selector;
    uint8_t IST;
    uint8_t Type_Attr;
    uint16_t OffsetMiddle;
    uint32_t OffsetHigh;
    uint32_t Zero;
} __attribute__((packed)) IDTEntry;

typedef struct {
    uint16_t Limit;
    uint64_t Pointer;
} __attribute__((packed)) IDTDescriptor;

void idt_init(void);
void idt_set_descriptor(uint8_t vector, void* isr, uint8_t flags);
