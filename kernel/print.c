#include "print.h"
#include <stdarg.h>
#include <stdint.h>
#include "graphics.h"

// Simple memmove implementation since we don't have a C library
static void memmove_local(void* dest, const void* src, uint64_t n) {
    if (n == 0 || dest == src) return;
    uint8_t* d = (uint8_t*)dest;
    const uint8_t* s = (const uint8_t*)src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else {
        uint8_t* lasts = (uint8_t*)s + (n - 1);
        uint8_t* lastd = (uint8_t*)d + (n - 1);
        while (n--) *lastd-- = *lasts--;
    }
}

// Simple memset implementation
static void memset_local(void* ptr, int value, uint64_t num) {
    uint8_t* p = (uint8_t*)ptr;
    while (num--) *p++ = (uint8_t)value;
}

static BootInfo* global_boot_info = 0;
static uint32_t cursor_x = 0;
static uint32_t cursor_y = 0;
static uint32_t color_fg = 0x00FFFFFF; // White
static uint32_t color_bg = 0x00000000; // Black (Transparent)

void print_init(BootInfo* bootInfo) {
    global_boot_info = bootInfo;
    cursor_x = 0;
    cursor_y = 0;
}

void set_text_color(uint32_t fg, uint32_t bg) {
    color_fg = fg;
    color_bg = bg;
}

void set_cursor(uint32_t x, uint32_t y) {
    cursor_x = x;
    cursor_y = y;
}

static void scroll(void) {
    if (!global_boot_info) return;
    Framebuffer* fb = global_boot_info->payload.framebuffer;
    PSF1_Font* font = global_boot_info->payload.font;
    uint32_t* back_buffer = get_back_buffer();

    uint32_t fontHeight = font->header->chars_size;
    
    // Check if we need to scroll
    if (cursor_y + fontHeight <= fb->Height) return;

    // Calculate how many bytes to move
    uint64_t bytesPerLine = fb->PixelsPerScanLine * 4;
    uint64_t totalBytesToMove = bytesPerLine * (fb->Height - fontHeight);
    
    // Move everything up by one fontHeight
    if (back_buffer) {
        memmove_local(
            back_buffer,
            (uint8_t*)back_buffer + (bytesPerLine * fontHeight),
            totalBytesToMove
        );
        memset_local(
            (uint8_t*)back_buffer + totalBytesToMove,
            0, // Clear to black
            bytesPerLine * fontHeight
        );
    } else {
        memmove_local(
            fb->BaseAddress,
            (uint8_t*)fb->BaseAddress + (bytesPerLine * fontHeight),
            totalBytesToMove
        );
        memset_local(
            (uint8_t*)fb->BaseAddress + totalBytesToMove,
            0, // Clear to black
            bytesPerLine * fontHeight
        );
    }

    // Reset cursor to the start of the bottom line
    cursor_y -= fontHeight;
}

void putchar(char c) {
    if (!global_boot_info) return;
    
    Framebuffer* fb = global_boot_info->payload.framebuffer;
    PSF1_Font* font = global_boot_info->payload.font;

    // Handle newline
    if (c == '\n') {
        cursor_x = 0;
        cursor_y += font->header->chars_size; // Move down by font height
        scroll();
        return;
    }

    // Wrap around if we hit the edge
    if (cursor_x + 8 > fb->Width) {
        cursor_x = 0;
        cursor_y += font->header->chars_size;
        scroll();
    }

    // Get the address of the specific character glyph in the font buffer
    uint8_t* glyph = (uint8_t*)font->glyphBuffer + (c * font->header->chars_size);

    for (uint32_t y = 0; y < font->header->chars_size; y++) {
        for (uint32_t x = 0; x < 8; x++) {
            // Check if the bit is set in the font bitmap
            if ((glyph[y] >> (7 - x)) & 1) { // PSF fonts are drawn left-to-right MSB first
                draw_pixel(cursor_x + x, cursor_y + y, color_fg);
            } else {
                if (color_bg != 0) {
                    draw_pixel(cursor_x + x, cursor_y + y, color_bg);
                }
            }
        }
    }

    cursor_x += 8; // Move cursor right by 8 pixels (PSF1 width is always 8)
}

static void print_string(const char* str) {
    if (!str) str = "(null)";
    while (*str) {
        putchar(*str++);
    }
}

static void print_hex(uint64_t val) {
    char buf[20];
    int i = 18;
    buf[19] = '\0';
    if (val == 0) {
        putchar('0');
        return;
    }
    while (val > 0) {
        int rem = val % 16;
        if (rem < 10) buf[i--] = '0' + rem;
        else buf[i--] = 'a' + (rem - 10);
        val /= 16;
    }
    print_string(&buf[i + 1]);
}

static void print_dec(int64_t val) {
    char buf[20];
    int i = 18;
    buf[19] = '\0';
    if (val == 0) {
        putchar('0');
        return;
    }
    int is_neg = 0;
    if (val < 0) {
        is_neg = 1;
        val = -val;
    }
    while (val > 0) {
        buf[i--] = '0' + (val % 10);
        val /= 10;
    }
    if (is_neg) buf[i--] = '-';
    print_string(&buf[i + 1]);
}

static void print_uint(uint64_t val) {
    char buf[21];
    int i = 19;
    buf[20] = '\0';
    if (val == 0) {
        putchar('0');
        return;
    }
    while (val > 0) {
        buf[i--] = '0' + (val % 10);
        val /= 10;
    }
    print_string(&buf[i + 1]);
}

void printf(const char* format, ...) {
    if (!format) return;
    va_list args;
    va_start(args, format);

    for (int i = 0; format[i] != '\0'; i++) {
        if (format[i] == '%') {
            i++;
            // Handle optional length modifiers (like 'l' or 'll')
            int is_long = 0;
            while (format[i] == 'l') {
                is_long++;
                i++;
            }

            switch (format[i]) {
                case 's':
                    print_string(va_arg(args, const char*));
                    break;
                case 'd':
                    if (is_long >= 2) {
                        print_dec(va_arg(args, int64_t));
                    } else if (is_long == 1) {
                        print_dec(va_arg(args, long));
                    } else {
                        print_dec(va_arg(args, int));
                    }
                    break;
                case 'x':
                    // Just print 64-bit hex anyway for simplicity in this OS
                    print_hex(va_arg(args, uint64_t));
                    break;
                case 'u':
                    if (is_long >= 2) {
                        print_uint(va_arg(args, uint64_t));
                    } else if (is_long == 1) {
                        print_uint(va_arg(args, unsigned long));
                    } else {
                        print_uint(va_arg(args, unsigned int));
                    }
                    break;
                case '%':
                    putchar('%');
                    break;
                default:
                    putchar('%');
                    putchar(format[i]);
                    break;
            }
        } else {
            putchar(format[i]);
        }
    }

    va_end(args);
}
