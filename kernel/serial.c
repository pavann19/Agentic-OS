#include "serial.h"
#include "io.h"

#define COM1 0x3F8

void serial_init(void) {
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x80);
    outb(COM1 + 0, 0x03);
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03);
    outb(COM1 + 2, 0xC7);
    outb(COM1 + 4, 0x0B);
}

int serial_is_ready(void) {
    return (inb(COM1 + 5) & 0x20) != 0;
}

void serial_write_char(char c) {
    while (!serial_is_ready()) {}
    outb(COM1, (uint8_t)c);
}

void serial_write_string(const char* str) {
    if (!str) str = "(null)";
    while (*str) {
        if (*str == '\n') serial_write_char('\r');
        serial_write_char(*str++);
    }
}
