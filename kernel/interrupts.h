#pragma once
#include <stdint.h>

// Struct for the state of the CPU when the interrupt was called
typedef struct {
    uint64_t rip;
    uint64_t cs;
    uint64_t rflags;
    uint64_t rsp;
    uint64_t ss;
} __attribute__((packed)) InterruptFrame;

void interrupts_init(void);
