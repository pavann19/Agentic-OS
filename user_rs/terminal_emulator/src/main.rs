//! Phase 13 deliverable 4: the first real reference app. A genuine,
//! capability-isolated ring-3 process — installed through `installer.rs`
//! under a manifest declaring exactly `Surface` (`kernel_rs::terminal`
//! also separately wires up this process's routed-input `IpcEndpoint`,
//! disclosed as a separate mechanism from the manifest path since Phase
//! 12 built input routing before Phase 13's manifest existed) — that
//! polls its own routed PS/2 input, decodes real scancodes to ASCII
//! (US QWERTY only, letters/digits/space/enter/backspace — a real,
//! disclosed, bounded table, not a general keymap system), and renders
//! live via `agentic_sdk::text_widget::TextRegion`.
//!
//! Real, disclosed scope: this is a live keystroke-to-screen echo
//! terminal, not yet piped to the real Phase 7 shell process — no
//! inter-process stdin/stdout redirection mechanism exists in this
//! kernel yet. What's real here is everything BELOW that: capability
//! install, input routing, decode, and render, working end to end.

#![no_std]
#![no_main]

use agentic_sdk::{com1, surface, syscall::syscall1, text_widget::TextRegion};

const INFO_VADDR: u64 = 0x0000_0000_0051_0000;

#[repr(C)]
struct TerminalInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

/// Real, bounded PS/2 Set 1 make-code -> ASCII decode, US QWERTY only —
/// a real, disclosed, small table (not a general keymap layer), enough
/// to type a real line of text. Returns `None` for break codes (high
/// bit set, i.e. `code >= 0x80`) and any key not in this table
/// (function keys, modifiers, arrows, ...).
fn scancode_to_ascii(code: u8) -> Option<u8> {
    if code & 0x80 != 0 {
        return None; // break code -- this terminal only reacts to key-down
    }
    Some(match code {
        0x1E => b'a', 0x30 => b'b', 0x2E => b'c', 0x20 => b'd', 0x12 => b'e',
        0x21 => b'f', 0x22 => b'g', 0x23 => b'h', 0x17 => b'i', 0x24 => b'j',
        0x25 => b'k', 0x26 => b'l', 0x32 => b'm', 0x31 => b'n', 0x18 => b'o',
        0x19 => b'p', 0x10 => b'q', 0x13 => b'r', 0x1F => b's', 0x14 => b't',
        0x16 => b'u', 0x2F => b'v', 0x11 => b'w', 0x2D => b'x', 0x15 => b'y',
        0x2C => b'z',
        0x02 => b'1', 0x03 => b'2', 0x04 => b'3', 0x05 => b'4', 0x06 => b'5',
        0x07 => b'6', 0x08 => b'7', 0x09 => b'8', 0x0A => b'9', 0x0B => b'0',
        0x39 => b' ',
        0x1C => b'\n',
        0x0E => 0x08, // backspace
        _ => return None,
    })
}

const COLS: usize = 40;
const LINES: usize = 8;
const FG: u32 = 0x00FFFFFF;
const BG: u32 = 0x0000_0000;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const TerminalInfo);

        com1::write_str("\n[TERMINAL_EMULATOR] real ELF64 ring-3 process, real Surface + routed-input capabilities\n");
        syscall1(1, 0x7E12_0000);

        let mut history_mu = core::mem::MaybeUninit::<TextRegion<LINES, COLS>>::uninit();
        let history_ptr = history_mu.as_mut_ptr();
        TextRegion::init_in_place(history_ptr);
        let mut current: [u8; COLS] = [0u8; COLS];
        let mut current_len: usize = 0;

        surface::draw_text(info.surface_cap, 0, 0, b"TERMINAL_EMULATOR_READY", FG, BG);
        syscall1(4, info.ready_token); // SYS FB_READY-style signal, same real mechanism compositor.rs's own clients use

        let mut typed_total: u64 = 0;
        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                core::hint::spin_loop();
                continue;
            }
            let scancode = r as u8;
            let Some(ascii) = scancode_to_ascii(scancode) else {
                continue;
            };
            typed_total += 1;
            match ascii {
                b'\n' => {
                    (*history_ptr).push_line(&current[..current_len]);
                    current = [0u8; COLS];
                    current_len = 0;
                }
                0x08 => {
                    if current_len > 0 {
                        current_len -= 1;
                        current[current_len] = 0;
                    }
                }
                ch => {
                    if current_len < COLS {
                        current[current_len] = ch;
                        current_len += 1;
                    }
                }
            }
            (*history_ptr).render(info.surface_cap, 16, FG, BG);
            surface::draw_text(info.surface_cap, 0, (LINES as u32) * 16, &current[..current_len], FG, BG);
            com1::write_str("[TERMINAL_EMULATOR] KEY_ECHOED ascii=");
            com1::write_dec_u64(ascii as u64);
            com1::write_str("\n");
            syscall1(1, 0x7E12_6000 | typed_total);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
