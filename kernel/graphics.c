#include "graphics.h"
#include "memory.h"
#include "print.h"
#include "paging.h"
#include "klog.h"

static Framebuffer* fb = 0;
static uint32_t* back_buffer = 0;
static uint64_t buffer_size_bytes = 0;

void graphics_early_init(BootInfo* bootInfo) {
    fb = bootInfo->payload.framebuffer;
    buffer_size_bytes = fb->BufferSize;
}

void graphics_init(BootInfo* bootInfo) {
    if (!fb) graphics_early_init(bootInfo);
    
    // Calculate how many pages we need for the back buffer
    uint64_t pages_needed = buffer_size_bytes / PAGE_SIZE;
    if (buffer_size_bytes % PAGE_SIZE != 0) pages_needed++;

    // Use VMM to allocate contiguous virtual memory!
    back_buffer = (uint32_t*)vmm_alloc_pages(pages_needed);
    
    for (uint64_t i = 0; i < buffer_size_bytes / 4; i++) {
        back_buffer[i] = 0;
    }
}

void draw_pixel(uint32_t x, uint32_t y, uint32_t color) {
    if (!fb || x >= fb->Width || y >= fb->Height) return;
    
    if (back_buffer) {
        back_buffer[y * fb->PixelsPerScanLine + x] = color;
    } else {
        // Fallback to hardware buffer if back buffer isn't initialized yet
        uint32_t* hw_buffer = (uint32_t*)fb->BaseAddress;
        hw_buffer[y * fb->PixelsPerScanLine + x] = color;
    }
}

void draw_rect(uint32_t x, uint32_t y, uint32_t width, uint32_t height, uint32_t color) {
    for (uint32_t dy = 0; dy < height; dy++) {
        for (uint32_t dx = 0; dx < width; dx++) {
            draw_pixel(x + dx, y + dy, color);
        }
    }
}

void swap_buffers(void) {
    if (!back_buffer || !fb) return;
    uint32_t* hw_buffer = (uint32_t*)fb->BaseAddress;
    for (uint64_t i = 0; i < buffer_size_bytes / 4; i++) {
        hw_buffer[i] = back_buffer[i];
    }
}

uint32_t get_screen_width(void) { return fb ? fb->Width : 0; }
uint32_t get_screen_height(void) { return fb ? fb->Height : 0; }
uint32_t* get_back_buffer(void) { return back_buffer; }
