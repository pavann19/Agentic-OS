#include "memory.h"
#include "print.h"
#include "klog.h"

static uint8_t* used_bitmap = 0;
static uint8_t* reserved_bitmap = 0;
static PmmStats pmm_stats = {0};
static uint64_t total_span_pages = 0;
static uint64_t last_scanned_page = 0;

static void bitmap_set(uint8_t* bitmap, uint64_t index) {
    bitmap[index / 8] |= (1 << (index % 8));
}

static void bitmap_clear(uint8_t* bitmap, uint64_t index) {
    bitmap[index / 8] &= ~(1 << (index % 8));
}

static int bitmap_test(uint8_t* bitmap, uint64_t index) {
    return (bitmap[index / 8] & (1 << (index % 8))) != 0;
}

PmmStats pmm_get_stats(void) {
    return pmm_stats;
}

void pmm_dump_stats(void) {
    klog_info("--- PMM Stats ---");
    klog_info("Total Span: %llu MB", pmm_stats.total_physical_span / (1024 * 1024));
    klog_info("Total Usable: %llu MB", pmm_stats.total_usable_memory / (1024 * 1024));
    klog_info("Reserved: %llu KB", pmm_stats.reserved_memory / 1024);
    klog_info("Free Pages: %llu", pmm_stats.free_pages);
    klog_info("Used Pages: %llu", pmm_stats.used_pages);
}

void pmm_reserve_range(uint64_t start, uint64_t length, const char* reason) {
    uint64_t start_page = start / PAGE_SIZE;
    uint64_t pages = (length + PAGE_SIZE - 1) / PAGE_SIZE;
    
    if (start % PAGE_SIZE != 0) {
        pages = (length + (start % PAGE_SIZE) + PAGE_SIZE - 1) / PAGE_SIZE;
    }

    for (uint64_t p = 0; p < pages; p++) {
        uint64_t idx = start_page + p;
        if (idx >= total_span_pages) continue;
        
        if (!bitmap_test(reserved_bitmap, idx)) {
            bitmap_set(reserved_bitmap, idx);
            if (!bitmap_test(used_bitmap, idx)) {
                bitmap_set(used_bitmap, idx);
                pmm_stats.free_pages--;
            }
            pmm_stats.reserved_memory += PAGE_SIZE;
        }
    }
    klog_info("Reserved %llu pages for %s (Start: 0x%llx)", pages, reason, start);
}

void pmm_init(BootInfo* bootInfo) {
    uint64_t highest_address = 0;
    
    EFI_MEMORY_DESCRIPTOR_STRUCT* map = bootInfo->payload.memory_map;
    uint64_t entries = bootInfo->payload.memory_map_size / bootInfo->payload.memory_map_descriptor_size;
    
    // 1. First pass: Find total physical span (highest address)
    for (uint64_t i = 0; i < entries; i++) {
        EFI_MEMORY_DESCRIPTOR_STRUCT* desc = (EFI_MEMORY_DESCRIPTOR_STRUCT*)((uint8_t*)map + (i * bootInfo->payload.memory_map_descriptor_size));
        uint64_t end_address = desc->PhysicalStart + (desc->NumberOfPages * PAGE_SIZE);
        if (end_address > highest_address) highest_address = end_address;
    }
    
    total_span_pages = highest_address / PAGE_SIZE;
    pmm_stats.total_physical_span = highest_address;
    
    uint64_t bitmap_size_bytes = total_span_pages / 8;
    if (total_span_pages % 8 != 0) bitmap_size_bytes++;

    // 2. Find a free segment large enough to hold our bitmaps
    void* used_bitmap_address = 0;
    void* reserved_bitmap_address = 0;
    uint64_t total_bitmap_size = bitmap_size_bytes * 2; // Two bitmaps
    
    for (uint64_t i = 0; i < entries; i++) {
        EFI_MEMORY_DESCRIPTOR_STRUCT* desc = (EFI_MEMORY_DESCRIPTOR_STRUCT*)((uint8_t*)map + (i * bootInfo->payload.memory_map_descriptor_size));
        
        if (desc->Type == EfiConventionalMemory && (desc->NumberOfPages * PAGE_SIZE) >= total_bitmap_size) {
            used_bitmap_address = (void*)desc->PhysicalStart;
            reserved_bitmap_address = (void*)((uint8_t*)desc->PhysicalStart + bitmap_size_bytes);
            break;
        }
    }

    if (used_bitmap_address == 0) {
        set_text_color(0x00FF0000, 0);
        printf("PANIC: Not enough free contiguous memory for PMM Bitmaps!\n");
        while(1) { __asm__ __volatile__("hlt"); }
    }
    
    used_bitmap = (uint8_t*)used_bitmap_address;
    reserved_bitmap = (uint8_t*)reserved_bitmap_address;
    
    // 3. Initialize all pages to used (0xFF) and not-reserved (0x00)
    for (uint64_t i = 0; i < bitmap_size_bytes; i++) {
        used_bitmap[i] = 0xFF; 
        reserved_bitmap[i] = 0x00;
    }
    
    pmm_stats.used_pages = 0;
    pmm_stats.free_pages = 0;
    pmm_stats.total_usable_memory = 0;
    pmm_stats.reserved_memory = 0;

    // 4. Free EfiConventionalMemory
    for (uint64_t i = 0; i < entries; i++) {
        EFI_MEMORY_DESCRIPTOR_STRUCT* desc = (EFI_MEMORY_DESCRIPTOR_STRUCT*)((uint8_t*)map + (i * bootInfo->payload.memory_map_descriptor_size));
        
        if (desc->Type == EfiConventionalMemory) {
            pmm_stats.total_usable_memory += desc->NumberOfPages * PAGE_SIZE;
            uint64_t start_page = desc->PhysicalStart / PAGE_SIZE;
            for (uint64_t p = 0; p < desc->NumberOfPages; p++) {
                bitmap_clear(used_bitmap, start_page + p);
                pmm_stats.free_pages++;
            }
        }
    }

    // 5. Explicitly Reserve Memory
    pmm_reserve_range(0, PAGE_SIZE, "Page Zero");
    pmm_reserve_range((uint64_t)used_bitmap_address, total_bitmap_size, "PMM Bitmaps");
    pmm_reserve_range(bootInfo->payload.kernel_physical_start, bootInfo->payload.kernel_physical_end - bootInfo->payload.kernel_physical_start, "Kernel");
    pmm_reserve_range((uint64_t)bootInfo, sizeof(BootInfo), "BootInfo");
    pmm_reserve_range((uint64_t)bootInfo->payload.framebuffer, sizeof(Framebuffer), "Framebuffer Struct");
    pmm_reserve_range((uint64_t)bootInfo->payload.font, sizeof(PSF1_Font), "Font Struct");
    pmm_reserve_range((uint64_t)bootInfo->payload.memory_map, bootInfo->payload.memory_map_size, "Memory Map Buffer");
    
    // Font Data
    if (bootInfo->payload.font->header) {
        pmm_reserve_range((uint64_t)bootInfo->payload.font->header, sizeof(PSF1_Header), "Font Header");
        uint32_t glyphBufferSize = bootInfo->payload.font->header->chars_size * 256;
        if (bootInfo->payload.font->header->mode == 1) glyphBufferSize *= 2;
        pmm_reserve_range((uint64_t)bootInfo->payload.font->glyphBuffer, glyphBufferSize, "Font Glyphs");
    }

    // Framebuffer MMIO
    pmm_reserve_range((uint64_t)bootInfo->payload.framebuffer->BaseAddress, bootInfo->payload.framebuffer->BufferSize, "Framebuffer MMIO");

    printf("Physical Memory Manager Initialized.\n");
    pmm_dump_stats();
}

void* pmm_alloc_page(void) {
    for (uint64_t i = last_scanned_page; i < total_span_pages; i++) {
        if (!bitmap_test(used_bitmap, i)) {
            bitmap_set(used_bitmap, i);
            pmm_stats.free_pages--;
            pmm_stats.used_pages++;
            last_scanned_page = i;
            
            void* ptr = (void*)(i * PAGE_SIZE);
            uint8_t* p = (uint8_t*)ptr;
            for (int j = 0; j < PAGE_SIZE; j++) p[j] = 0;
            return ptr;
        }
    }

    if (last_scanned_page > 0) {
        last_scanned_page = 0;
        return pmm_alloc_page();
    }

    set_text_color(0x00FF0000, 0);
    printf("OUT OF MEMORY: No physical pages left!\n");
    return 0;
}

void* pmm_alloc_pages(uint64_t count, uint32_t flags) {
    (void)flags;
    if (count == 0) return 0;

    uint64_t run_start = 0;
    uint64_t run_length = 0;

    for (uint64_t i = last_scanned_page; i < total_span_pages; i++) {
        if (!bitmap_test(used_bitmap, i)) {
            if (run_length == 0) run_start = i;
            run_length++;
            if (run_length == count) {
                for (uint64_t p = 0; p < count; p++) {
                    bitmap_set(used_bitmap, run_start + p);
                    pmm_stats.free_pages--;
                    pmm_stats.used_pages++;
                }
                last_scanned_page = run_start + count;

                uint8_t* base = (uint8_t*)(run_start * PAGE_SIZE);
                for (uint64_t j = 0; j < count * PAGE_SIZE; j++) {
                    base[j] = 0;
                }
                return base;
            }
        } else {
            run_length = 0;
        }
    }

    if (last_scanned_page > 0) {
        last_scanned_page = 0;
        return pmm_alloc_pages(count, flags);
    }

    set_text_color(0x00FF0000, 0);
    printf("OUT OF MEMORY: No contiguous run of %llu pages!\n", count);
    return 0;
}

void pmm_free_page(void* ptr) {
    uint64_t addr = (uint64_t)ptr;
    if (addr == 0) {
        klog_info("pmm_free_page: reject null/page 0");
        return;
    }
    if (addr % PAGE_SIZE != 0) {
        klog_info("pmm_free_page: reject unaligned (0x%llx)", addr);
        return; 
    }

    uint64_t page_index = addr / PAGE_SIZE;
    if (page_index >= total_span_pages) {
        klog_info("pmm_free_page: reject out-of-range (0x%llx)", addr);
        return;
    }

    if (bitmap_test(reserved_bitmap, page_index)) {
        klog_info("pmm_free_page: reject reserved (0x%llx)", addr);
        return;
    }

    if (!bitmap_test(used_bitmap, page_index)) {
        klog_info("pmm_free_page: reject already free (0x%llx)", addr);
        return;
    }

    bitmap_clear(used_bitmap, page_index);
    pmm_stats.free_pages++;
    pmm_stats.used_pages--;
    
    if (page_index < last_scanned_page) {
        last_scanned_page = page_index;
    }
}
