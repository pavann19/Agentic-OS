#pragma once
#include <stdint.h>
#include <bootinfo.h>

// Initialize the print engine with the boot info
void print_init(BootInfo* bootInfo);

// Core drawing functions
void putchar(char c);
void printf(const char* format, ...);
void set_cursor(uint32_t x, uint32_t y);

// Color management
void set_text_color(uint32_t fg, uint32_t bg);
