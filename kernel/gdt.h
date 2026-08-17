#pragma once
#include <stdint.h>

typedef struct {
    uint16_t LimitLow;
    uint16_t BaseLow;
    uint8_t BaseMiddle;
    uint8_t Access;
    uint8_t FlagsLimitHigh;
    uint8_t BaseHigh;
} __attribute__((packed)) GDTEntry;

typedef struct {
    uint16_t Limit;
    uint64_t Pointer;
} __attribute__((packed)) GDTDescriptor;

void gdt_init(void);
