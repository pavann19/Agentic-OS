#include "keyboard.h"
#include "io.h"
#include "pic.h"
#include "print.h"
#include "graphics.h"

// Basic US QWERTY Scancode Set 1 Lookup Table (Lower Case)
const char scancode_to_char[] = {
    0, 0, '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-', '=', 0, // 0x0E is Backspace
    '\t', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p', '[', ']', '\n',
    0, // 0x1D is Ctrl
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', '\'', '`',
    0, // 0x2A is Left Shift
    '\\', 'z', 'x', 'c', 'v', 'b', 'n', 'm', ',', '.', '/',
    0, // 0x36 is Right Shift
    '*', 
    0, // 0x38 is Alt
    ' ', // 0x39 is Space
};

__attribute__((interrupt)) void Keyboard_Handler(InterruptFrame* frame) {
    (void)frame;
    uint8_t scancode = inb(0x60); // Read from keyboard data port

    // If the top bit is set, it's a "key release" event. We only care about key presses for now.
    if (!(scancode & 0x80)) { 
        if (scancode < sizeof(scancode_to_char)) {
            char c = scancode_to_char[scancode];
            if (c != 0) {
                putchar(c);
                swap_buffers(); // Force the screen to update with the new character
            }
        }
    }

    // Tell the PIC we are done processing the interrupt
    pic_send_eoi(1);
}
