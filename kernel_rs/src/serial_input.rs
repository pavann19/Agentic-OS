//! Kernel-level Serial Input Bridge: reads incoming bytes from COM1
//! (16550 UART at 0x3F8) and translates them into PS/2 Set 1 scancodes
//! routed directly via `input_routing::deliver_key_event()`.
//!
//! Essential for Hyper-V Generation 2: Hyper-V Gen 2 has NO Intel 8042
//! PS/2 controller at port 0x60/0x64 (reading returns 0xFF, IRQ1/12 never fire).
//! This bridge allows any host terminal connected to \\.\pipe\AgenticOS_COM1
//! to type directly into the focused GUI window (Terminal emulator, Text editor,
//! Compositor) and control the mouse cursor.

use crate::{input_routing, klog_info, serial, thread, window_manager};

enum EscState {
    Normal,
    SawEsc,
    SawBracket,
}

fn ascii_to_ps2_scancode(c: u8) -> Option<u8> {
    let lower = if (b'A'..=b'Z').contains(&c) {
        c + (b'a' - b'A')
    } else {
        c
    };

    match lower {
        b'a' => Some(0x1E),
        b'b' => Some(0x30),
        b'c' => Some(0x2E),
        b'd' => Some(0x20),
        b'e' => Some(0x12),
        b'f' => Some(0x21),
        b'g' => Some(0x22),
        b'h' => Some(0x23),
        b'i' => Some(0x17),
        b'j' => Some(0x24),
        b'k' => Some(0x25),
        b'l' => Some(0x26),
        b'm' => Some(0x32),
        b'n' => Some(0x31),
        b'o' => Some(0x18),
        b'p' => Some(0x19),
        b'q' => Some(0x10),
        b'r' => Some(0x13),
        b's' => Some(0x1F),
        b't' => Some(0x14),
        b'u' => Some(0x16),
        b'v' => Some(0x2F),
        b'w' => Some(0x11),
        b'x' => Some(0x2D),
        b'y' => Some(0x15),
        b'z' => Some(0x2C),
        b'1' => Some(0x02),
        b'2' => Some(0x03),
        b'3' => Some(0x04),
        b'4' => Some(0x05),
        b'5' => Some(0x06),
        b'6' => Some(0x07),
        b'7' => Some(0x08),
        b'8' => Some(0x09),
        b'9' => Some(0x0A),
        b'0' => Some(0x0B),
        b' ' => Some(0x39),
        b'\r' | b'\n' => Some(0x1C), // Enter
        0x08 | 0x7F => Some(0x0E),   // Backspace / DEL
        _ => None,
    }
}

pub fn spawn() {
    thread::spawn(serial_input_thread);
}

extern "C" fn serial_input_thread() {
    klog_info!("SERIAL_INPUT_BRIDGE_STARTED");
    let mut state = EscState::Normal;

    loop {
        if let Some(byte) = serial::read_char() {
            match state {
                EscState::Normal => {
                    if byte == 0x1B {
                        state = EscState::SawEsc;
                    } else if let Some(scancode) = ascii_to_ps2_scancode(byte) {
                        input_routing::deliver_key_event(scancode);
                    }
                }
                EscState::SawEsc => {
                    if byte == b'[' {
                        state = EscState::SawBracket;
                    } else {
                        state = EscState::Normal;
                    }
                }
                EscState::SawBracket => {
                    state = EscState::Normal;
                    match byte {
                        b'A' => {
                            // Up Arrow -> mouse move up 20px
                            window_manager::report_mouse(0, -20, false);
                        }
                        b'B' => {
                            // Down Arrow -> mouse move down 20px
                            window_manager::report_mouse(0, 20, false);
                        }
                        b'C' => {
                            // Right Arrow -> PS/2 Set 1 extended 0xE0 0x4D
                            input_routing::deliver_key_event(0xE0);
                            input_routing::deliver_key_event(0x4D);
                        }
                        b'D' => {
                            // Left Arrow -> PS/2 Set 1 extended 0xE0 0x4B
                            input_routing::deliver_key_event(0xE0);
                            input_routing::deliver_key_event(0x4B);
                        }
                        _ => {}
                    }
                }
            }
        } else {
            // Cooperative pause when no byte waiting
            thread::schedule();
        }
    }
}
