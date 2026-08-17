#include <stdint.h>
#include "print.h"
#include <bootinfo.h>
#include "gdt.h"
#include "idt.h"
#include "interrupts.h"
#include "memory.h"
#include "paging.h"
#include "graphics.h"
#include "desktop.h"
#include "klog.h"

void kernel_main(BootInfo* bootInfo) {
    klog_init();
    if (!bootInfo || bootInfo->magic != BOOTINFO_MAGIC || bootInfo->version != BOOTINFO_VERSION) {
        panic("invalid boot info");
    }
    klog_info("KERNEL_ENTER");
    klog_info("BootInfo version=%u size=%u", bootInfo->version, bootInfo->size);

    // 1. Early init graphics and print so we can debug the boot process!
    graphics_early_init(bootInfo);
    print_init(bootInfo);

    // Clear the screen to a deep blue manually (hardware buffer)
    draw_rect(0, 0, get_screen_width(), get_screen_height(), 0x000B1A3A);
    set_cursor(0, 0);
    set_text_color(0x00FFFFFF, 0x000B1A3A); // White on Blue
    
    printf("Kernel Booting...\n");
    klog_info("Kernel framebuffer ready");
    printf("Initializing GDT, IDT, and Interrupts...\n");
    gdt_init();
    idt_init();
    interrupts_init();

    printf("Initializing Physical Memory Manager...\n");
    klog_info("PMM_INIT_START");
    pmm_init(bootInfo);
    klog_info("PMM_INIT_DONE");
    
    printf("Initializing Virtual Memory (Paging)...\n");
    klog_info("VMM_INIT_START");
    Paging_Init(bootInfo);
    klog_info("VMM_INIT_DONE");

    printf("Virtual Memory active. Initializing Double Buffered Graphics...\n");
    graphics_init(bootInfo);

    // 2. Draw the Desktop UI (this goes to the back buffer)
    draw_desktop();

    // 3. Print text into our window!
    uint32_t width = get_screen_width();
    uint32_t height = get_screen_height();
    uint32_t win_width = 600;
    uint32_t win_height = 400;
    uint32_t win_x = (width - win_width) / 2;
    uint32_t win_y = (height - win_height) / 2;

    set_cursor(win_x + 10, win_y + 7);
    set_text_color(0x00FFFFFF, 0); // White on transparent
    printf("Agentic OS - First Window!");

    set_cursor(win_x + 20, win_y + 50);
    printf("Welcome to the GUI!");

    set_cursor(win_x + 20, win_y + 80);
    set_text_color(0x00A6E3A1, 0); // Green text
    printf("Double Buffering Active.");

    set_cursor(win_x + 20, win_y + 110);
    set_text_color(0x00FFFFFF, 0);
    printf("Try typing on your keyboard: ");

    // 4. Swap buffers to push the drawn frame to the screen
    swap_buffers();

    // 5. Enable hardware interrupts
    __asm__ __volatile__("sti");

    // Halt the CPU forever, waiting for interrupts
    while (1) {
        __asm__ __volatile__("hlt");
    }
}
