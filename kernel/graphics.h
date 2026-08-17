#pragma once
#include <stdint.h>
#include <bootinfo.h>

void graphics_early_init(BootInfo* bootInfo);
void graphics_init(BootInfo* bootInfo);
void draw_pixel(uint32_t x, uint32_t y, uint32_t color);
void draw_rect(uint32_t x, uint32_t y, uint32_t width, uint32_t height, uint32_t color);
void swap_buffers(void);

uint32_t get_screen_width(void);
uint32_t get_screen_height(void);
uint32_t* get_back_buffer(void);
