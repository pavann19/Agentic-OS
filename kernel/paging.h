#pragma once
#include <stdint.h>
#include <bootinfo.h>

enum PAGE_DIR_FLAGS {
    PAGE_PRESENT = 1,
    PAGE_READ_WRITE = 2,
    PAGE_USER = 4,
};

typedef struct {
    uint64_t entries[512];
} PageTable;

void Paging_Init(BootInfo* bootInfo);
void MapMemory(void* virtualMemory, void* physicalMemory);
void* vmm_alloc_pages(uint64_t pages);
