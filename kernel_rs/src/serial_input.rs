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

fn ascii_to_ps2(c: u8) -> Option<(u8, bool)> {
    match c {
        b'a'..=b'z' => {
            let scancode = match c {
                b'a' => 0x1E, b'b' => 0x30, b'c' => 0x2E, b'd' => 0x20, b'e' => 0x12,
                b'f' => 0x21, b'g' => 0x22, b'h' => 0x23, b'i' => 0x17, b'j' => 0x24,
                b'k' => 0x25, b'l' => 0x26, b'm' => 0x32, b'n' => 0x31, b'o' => 0x18,
                b'p' => 0x19, b'q' => 0x10, b'r' => 0x13, b's' => 0x1F, b't' => 0x14,
                b'u' => 0x16, b'v' => 0x2F, b'w' => 0x11, b'x' => 0x2D, b'y' => 0x15,
                b'z' => 0x2C, _ => return None,
            };
            Some((scancode, false))
        }
        b'A'..=b'Z' => {
            let lower = c + (b'a' - b'A');
            let scancode = match lower {
                b'a' => 0x1E, b'b' => 0x30, b'c' => 0x2E, b'd' => 0x20, b'e' => 0x12,
                b'f' => 0x21, b'g' => 0x22, b'h' => 0x23, b'i' => 0x17, b'j' => 0x24,
                b'k' => 0x25, b'l' => 0x26, b'm' => 0x32, b'n' => 0x31, b'o' => 0x18,
                b'p' => 0x19, b'q' => 0x10, b'r' => 0x13, b's' => 0x1F, b't' => 0x14,
                b'u' => 0x16, b'v' => 0x2F, b'w' => 0x11, b'x' => 0x2D, b'y' => 0x15,
                b'z' => 0x2C, _ => return None,
            };
            Some((scancode, true))
        }
        b'1' => Some((0x02, false)), b'!' => Some((0x02, true)),
        b'2' => Some((0x03, false)), b'@' => Some((0x03, true)),
        b'3' => Some((0x04, false)), b'#' => Some((0x04, true)),
        b'4' => Some((0x05, false)), b'$' => Some((0x05, true)),
        b'5' => Some((0x06, false)), b'%' => Some((0x06, true)),
        b'6' => Some((0x07, false)), b'^' => Some((0x07, true)),
        b'7' => Some((0x08, false)), b'&' => Some((0x08, true)),
        b'8' => Some((0x09, false)), b'*' => Some((0x09, true)),
        b'9' => Some((0x0A, false)), b'(' => Some((0x0A, true)),
        b'0' => Some((0x0B, false)), b')' => Some((0x0B, true)),
        b'-' => Some((0x0C, false)), b'_' => Some((0x0C, true)),
        b'=' => Some((0x0D, false)), b'+' => Some((0x0D, true)),
        b'[' => Some((0x1A, false)), b'{' => Some((0x1A, true)),
        b']' => Some((0x1B, false)), b'}' => Some((0x1B, true)),
        b';' => Some((0x27, false)), b':' => Some((0x27, true)),
        b'\'' => Some((0x28, false)), b'"' => Some((0x28, true)),
        b'`' => Some((0x29, false)), b'~' => Some((0x29, true)),
        b'\\' => Some((0x2B, false)), b'|' => Some((0x2B, true)),
        b',' => Some((0x33, false)), b'<' => Some((0x33, true)),
        b'.' => Some((0x34, false)), b'>' => Some((0x34, true)),
        b'/' => Some((0x35, false)), b'?' => Some((0x35, true)),
        b' ' => Some((0x39, false)),
        b'\t' => Some((0x0F, false)),
        b'\r' | b'\n' => Some((0x1C, false)),
        0x08 | 0x7F => Some((0x0E, false)),
        0x1B => Some((0x01, false)),
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
                    } else if let Some((scancode, shifted)) = ascii_to_ps2(byte) {
                        if shifted {
                            input_routing::deliver_key_event(0x2A); // Shift make
                            input_routing::deliver_key_event(scancode);
                            input_routing::deliver_key_event(0xAA); // Shift break
                        } else {
                            input_routing::deliver_key_event(scancode);
                        }
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
