#include "klog.h"
#include "serial.h"
#include "print.h"
#include <stdarg.h>

static void klog_putchar(char c) {
    serial_write_char(c);
    putchar(c);
}

static void klog_string(const char* str) {
    if (!str) str = "(null)";
    while (*str) {
        if (*str == '\n') serial_write_char('\r');
        klog_putchar(*str++);
    }
}

static void klog_hex(uint64_t value) {
    char buf[17];
    for (int i = 15; i >= 0; i--) {
        uint8_t nibble = value & 0xF;
        buf[i] = (nibble < 10) ? (char)('0' + nibble) : (char)('a' + nibble - 10);
        value >>= 4;
    }
    buf[16] = 0;
    klog_string("0x");
    klog_string(buf);
}

static void klog_uint(uint64_t value) {
    char buf[21];
    int i = 19;
    buf[20] = 0;
    if (value == 0) {
        klog_putchar('0');
        return;
    }
    while (value && i >= 0) {
        buf[i--] = (char)('0' + (value % 10));
        value /= 10;
    }
    klog_string(&buf[i + 1]);
}

static void klog_int(int64_t value) {
    if (value < 0) {
        klog_putchar('-');
        klog_uint((uint64_t)(-value));
    } else {
        klog_uint((uint64_t)value);
    }
}

static void klog_vprintf(const char* format, va_list args) {
    if (!format) {
        klog_string("(null)");
        return;
    }

    for (int i = 0; format[i]; i++) {
        if (format[i] != '%') {
            klog_putchar(format[i]);
            continue;
        }

        i++;
        int long_count = 0;
        while (format[i] == 'l') {
            long_count++;
            i++;
        }

        switch (format[i]) {
            case 's':
                klog_string(va_arg(args, const char*));
                break;
            case 'd':
                klog_int(long_count ? va_arg(args, int64_t) : va_arg(args, int));
                break;
            case 'u':
                klog_uint(long_count ? va_arg(args, uint64_t) : va_arg(args, unsigned int));
                break;
            case 'x':
                klog_hex(va_arg(args, uint64_t));
                break;
            case '%':
                klog_putchar('%');
                break;
            default:
                klog_putchar('%');
                klog_putchar(format[i]);
                break;
        }
    }
}

static void klog_write(const char* level, const char* format, va_list args) {
    klog_string(level);
    klog_vprintf(format, args);
    klog_putchar('\n');
}

void klog_init(void) {
    serial_init();
    serial_write_string("KLOG_INIT\n");
}

void klog_info(const char* format, ...) {
    va_list args;
    va_start(args, format);
    klog_write("[INFO] ", format, args);
    va_end(args);
}

void klog_warn(const char* format, ...) {
    va_list args;
    va_start(args, format);
    klog_write("[WARN] ", format, args);
    va_end(args);
}

void klog_error(const char* format, ...) {
    va_list args;
    va_start(args, format);
    klog_write("[ERROR] ", format, args);
    va_end(args);
}

void panic(const char* reason) {
    __asm__ __volatile__("cli");
    klog_error("PANIC: %s", reason);
    while (1) {
        __asm__ __volatile__("hlt");
    }
}
