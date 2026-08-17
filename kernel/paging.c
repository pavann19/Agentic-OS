#include "paging.h"
#include "memory.h"
#include "print.h"

static PageTable* PML4;

static void PageMapIndexer(uint64_t virtualAddress, uint64_t* pml4_index, uint64_t* pdpt_index, uint64_t* pd_index, uint64_t* pt_index) {
    virtualAddress >>= 12;
    *pt_index = virtualAddress & 0x1ff;
    virtualAddress >>= 9;
    *pd_index = virtualAddress & 0x1ff;
    virtualAddress >>= 9;
    *pdpt_index = virtualAddress & 0x1ff;
    virtualAddress >>= 9;
    *pml4_index = virtualAddress & 0x1ff;
}

void MapMemory(void* virtualMemory, void* physicalMemory) {
    uint64_t pml4_index, pdpt_index, pd_index, pt_index;
    PageMapIndexer((uint64_t)virtualMemory, &pml4_index, &pdpt_index, &pd_index, &pt_index);

    // Get PDPT
    PageTable* pdpt;
    if (!(PML4->entries[pml4_index] & PAGE_PRESENT)) {
        pdpt = (PageTable*)pmm_alloc_page();
        for (int i=0; i<512; i++) pdpt->entries[i] = 0; // ZERO OUT GARBAGE!
        PML4->entries[pml4_index] = ((uint64_t)pdpt) | PAGE_PRESENT | PAGE_READ_WRITE;
    } else {
        pdpt = (PageTable*)(PML4->entries[pml4_index] & 0x000FFFFFFFFFF000);
    }

    // Get PD
    PageTable* pd;
    if (!(pdpt->entries[pdpt_index] & PAGE_PRESENT)) {
        pd = (PageTable*)pmm_alloc_page();
        for (int i=0; i<512; i++) pd->entries[i] = 0;
        pdpt->entries[pdpt_index] = ((uint64_t)pd) | PAGE_PRESENT | PAGE_READ_WRITE;
    } else {
        pd = (PageTable*)(pdpt->entries[pdpt_index] & 0x000FFFFFFFFFF000);
    }

    // Get PT
    PageTable* pt;
    if (!(pd->entries[pd_index] & PAGE_PRESENT)) {
        pt = (PageTable*)pmm_alloc_page();
        for (int i=0; i<512; i++) pt->entries[i] = 0;
        pd->entries[pd_index] = ((uint64_t)pt) | PAGE_PRESENT | PAGE_READ_WRITE;
    } else {
        pt = (PageTable*)(pd->entries[pd_index] & 0x000FFFFFFFFFF000);
    }

    // Map the actual page
    pt->entries[pt_index] = ((uint64_t)physicalMemory) | PAGE_PRESENT | PAGE_READ_WRITE;
}

void* vmm_alloc_pages(uint64_t pages) {
    // Start allocating virtual memory from 4GB upwards (away from identity mapped RAM)
    static uint64_t next_vaddr = 0x100000000; 
    void* start_vaddr = (void*)next_vaddr;
    
    for (uint64_t i = 0; i < pages; i++) {
        void* paddr = pmm_alloc_page();
        if (!paddr) return 0; // Out of memory
        MapMemory((void*)next_vaddr, paddr);
        next_vaddr += PAGE_SIZE;
    }
    return start_vaddr;
}

void Paging_Init(BootInfo* bootInfo) {
    // 1. Allocate the Root Page Table (PML4)
    PML4 = (PageTable*)pmm_alloc_page();
    for (int i=0; i<512; i++) PML4->entries[i] = 0; // ZERO OUT GARBAGE!

    // 2. Identity Map ALL physical RAM regions from the UEFI memory map!
    uint64_t entries = bootInfo->payload.memory_map_size / bootInfo->payload.memory_map_descriptor_size;
    for (uint64_t i = 0; i < entries; i++) {
        EFI_MEMORY_DESCRIPTOR_STRUCT* desc = (EFI_MEMORY_DESCRIPTOR_STRUCT*)((uint8_t*)bootInfo->payload.memory_map + (i * bootInfo->payload.memory_map_descriptor_size));
        
        uint64_t phys_start = desc->PhysicalStart;
        uint64_t size = desc->NumberOfPages * 4096;
        
        for (uint64_t p = phys_start; p < phys_start + size; p += 4096) {
            MapMemory((void*)p, (void*)p);
        }
    }

    // 3. Ensure the Framebuffer is mapped!
    uint64_t fbBase = (uint64_t)bootInfo->payload.framebuffer->BaseAddress;
    uint64_t fbSize = bootInfo->payload.framebuffer->BufferSize;
    
    for (uint64_t i = fbBase; i < fbBase + fbSize; i += PAGE_SIZE) {
        MapMemory((void*)i, (void*)i);
    }

    // 4. Load the PML4 into CR3 to activate Paging!
    __asm__ __volatile__("mov %0, %%cr3" : : "r" (PML4));
}
