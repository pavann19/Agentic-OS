#pragma once
#include <stdint.h>
#include <bootinfo.h>

#define PAGE_SIZE 4096

// UEFI Memory Types (from UEFI Spec)
#define EfiReservedMemoryType 0
#define EfiLoaderCode 1
#define EfiLoaderData 2
#define EfiBootServicesCode 3
#define EfiBootServicesData 4
#define EfiRuntimeServicesCode 5
#define EfiRuntimeServicesData 6
#define EfiConventionalMemory 7
#define EfiUnusableMemory 8
#define EfiACPIReclaimMemory 9
#define EfiACPIMemoryNVS 10
#define EfiMemoryMappedIO 11
#define EfiMemoryMappedIOPortSpace 12
#define EfiPalCode 13
#define EfiPersistentMemory 14

typedef struct {
    uint64_t total_physical_span; // bytes
    uint64_t total_usable_memory; // bytes
    uint64_t reserved_memory;     // bytes
    uint64_t free_pages;
    uint64_t used_pages;
} PmmStats;

void pmm_init(BootInfo* bootInfo);
void* pmm_alloc_page(void);
void* pmm_alloc_pages(uint64_t count, uint32_t flags);
void pmm_free_page(void* ptr);
void pmm_reserve_range(uint64_t start, uint64_t length, const char* reason);
PmmStats pmm_get_stats(void);
void pmm_dump_stats(void);
