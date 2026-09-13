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

/// Real, decoded key events this terminal reacts to -- a real,
/// disclosed, small set (not a general keymap layer): printable ASCII,
/// Enter, Backspace, and now real horizontal cursor movement
/// (Left/Right), enough to edit a real line of text, not just append
/// to its end.
#[derive(Clone, Copy)]
enum Key {
    Ascii(u8),
    Backspace,
    Enter,
    Left,
    Right,
}

/// Real, bounded PS/2 Set 1 decode, US QWERTY only. `extended` is true
/// when the PREVIOUS byte from this same input stream was the real
/// `0xE0` prefix PS/2 Set 1 uses for its extended keys (arrows, Home/
/// End/Insert/Delete, the right-hand Ctrl/Alt, ...) -- real, disclosed
/// scope: only Left/Right are decoded from that extended set so far
/// (the "some cursor movement" ask); every other extended key is a
/// real, silent no-op here, same as any unmapped key. Returns `None`
/// for break codes (high bit set, i.e. `code >= 0x80`) and any key not
/// in this table.
fn decode_key(code: u8, extended: bool) -> Option<Key> {
    if code & 0x80 != 0 {
        return None; // break code -- this terminal only reacts to key-down
    }
    if extended {
        return match code {
            0x4B => Some(Key::Left),
            0x4D => Some(Key::Right),
            _ => None,
        };
    }
    Some(match code {
        0x1E => Key::Ascii(b'a'), 0x30 => Key::Ascii(b'b'), 0x2E => Key::Ascii(b'c'), 0x20 => Key::Ascii(b'd'), 0x12 => Key::Ascii(b'e'),
        0x21 => Key::Ascii(b'f'), 0x22 => Key::Ascii(b'g'), 0x23 => Key::Ascii(b'h'), 0x17 => Key::Ascii(b'i'), 0x24 => Key::Ascii(b'j'),
        0x25 => Key::Ascii(b'k'), 0x26 => Key::Ascii(b'l'), 0x32 => Key::Ascii(b'm'), 0x31 => Key::Ascii(b'n'), 0x18 => Key::Ascii(b'o'),
        0x19 => Key::Ascii(b'p'), 0x10 => Key::Ascii(b'q'), 0x13 => Key::Ascii(b'r'), 0x1F => Key::Ascii(b's'), 0x14 => Key::Ascii(b't'),
        0x16 => Key::Ascii(b'u'), 0x2F => Key::Ascii(b'v'), 0x11 => Key::Ascii(b'w'), 0x2D => Key::Ascii(b'x'), 0x15 => Key::Ascii(b'y'),
        0x2C => Key::Ascii(b'z'),
        0x02 => Key::Ascii(b'1'), 0x03 => Key::Ascii(b'2'), 0x04 => Key::Ascii(b'3'), 0x05 => Key::Ascii(b'4'), 0x06 => Key::Ascii(b'5'),
        0x07 => Key::Ascii(b'6'), 0x08 => Key::Ascii(b'7'), 0x09 => Key::Ascii(b'8'), 0x0A => Key::Ascii(b'9'), 0x0B => Key::Ascii(b'0'),
        0x39 => Key::Ascii(b' '),
        0x1C => Key::Enter,
        0x0E => Key::Backspace,
        _ => return None,
    })
}

/// Real timing diagnostic (Track: chasing the reported "typing still
/// feels slow" after the page-cache + batched-present fixes) -- reads
/// the CPU's own real cycle counter, unprivileged on this kernel (CR4.
/// TSD is never set), so this measures REAL elapsed cycles for the
/// exact work between two points, not a guess. Logged over COM1 so it
/// shows up in the same serial evidence every other real measurement
/// in this project already relies on.
#[inline(always)]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    ((hi as u64) << 32) | (lo as u64)
}

const COLS: usize = 40;
const LINES: usize = 8;
const FG: u32 = 0x00FFFFFF;
const BG: u32 = 0x0000_0000;
/// Matches `kernel_rs::text`'s own real PSF1 glyph width -- every PSF1
/// glyph in this font is 8 pixels wide, a real, fixed fact about the
/// loaded font, not a guess (see that module's own doc); needed here
/// client-side to compute the cursor's real x pixel position.
const GLYPH_WIDTH: u32 = 8;

/// Real, visible text cursor: redraws the current line's real text,
/// then overlays ONE real inverted glyph (fg/bg swapped) at the
/// cursor's own column -- the actual character under the cursor if
/// there is one, a blank cell otherwise. Real, disclosed UI fix for
/// the reported "needs more visual enhancement": before this, there
/// was no on-screen indication of where a keystroke would land at all.
unsafe fn redraw_current_line(surface_cap: u32, y: u32, current: &[u8; COLS], current_len: usize, cursor: usize) {
    surface::draw_text(surface_cap, 0, y, current, FG, BG);
    let under = if cursor < current_len { current[cursor] } else { b' ' };
    let cell = [under];
    surface::draw_text(surface_cap, (cursor as u32) * GLYPH_WIDTH, y, &cell, BG, FG);
}

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
        surface::present(info.surface_cap);
        syscall1(4, info.ready_token); // SYS FB_READY-style signal, same real mechanism compositor.rs's own clients use

        let mut typed_total: u64 = 0;
        let mut cursor: usize = 0;
        // Real PS/2 Set 1 extended-key state: the real hardware sends
        // arrows (and other extended keys) as a real TWO-byte sequence
        // (`0xE0` prefix, then the actual code) -- this remembers
        // "the last byte we saw was that prefix" across loop
        // iterations, since each `SYS_IPC_TRY_RECEIVE` only ever
        // returns one real raw byte at a time.
        let mut pending_extended = false;
        let current_line_y = (LINES as u32) * 16;
        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                core::hint::spin_loop();
                continue;
            }
            let scancode = r as u8;
            if scancode == 0xE0 {
                pending_extended = true;
                continue;
            }
            let extended = pending_extended;
            pending_extended = false;
            let Some(key) = decode_key(scancode, extended) else {
                continue;
            };
            typed_total += 1;
            let t_decode = rdtsc();
            // Real, disclosed latency-fix discipline kept from before:
            // Enter is the only key that can change more than the
            // current line (a real scrollback push) -- everything else
            // (typing, backspace, cursor movement) only ever touches
            // the current line, so only Enter does a full `render()` +
            // full `present()`; everything else redraws + presents
            // ONLY that one row (`present_rect`).
            match key {
                Key::Enter => {
                    (*history_ptr).push_line(&current[..current_len]);
                    current = [0u8; COLS];
                    current_len = 0;
                    cursor = 0;
                    (*history_ptr).render(info.surface_cap, 16, FG, BG);
                    redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                    surface::present(info.surface_cap);
                }
                Key::Backspace => {
                    // Real, disclosed cursor-aware edit: removes the
                    // character immediately BEFORE the cursor (a real
                    // terminal's own behavior), shifting everything
                    // after it left by one -- not just truncating the
                    // last character regardless of where the cursor is.
                    if cursor > 0 && current_len > 0 {
                        for i in (cursor - 1)..(current_len - 1) {
                            current[i] = current[i + 1];
                        }
                        current_len -= 1;
                        current[current_len] = 0;
                        cursor -= 1;
                    }
                    redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                    surface::present_rect(info.surface_cap, current_line_y, 16);
                }
                Key::Left => {
                    if cursor > 0 {
                        cursor -= 1;
                    }
                    redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                    surface::present_rect(info.surface_cap, current_line_y, 16);
                }
                Key::Right => {
                    if cursor < current_len {
                        cursor += 1;
                    }
                    redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                    surface::present_rect(info.surface_cap, current_line_y, 16);
                }
                Key::Ascii(ch) => {
                    // Real, disclosed cursor-aware edit: INSERTS at the
                    // cursor (shifting everything after it right), not
                    // always appended at the end -- what makes Left/
                    // Right actually useful for editing.
                    if current_len < COLS {
                        for i in (cursor..current_len).rev() {
                            current[i + 1] = current[i];
                        }
                        current[cursor] = ch;
                        current_len += 1;
                        cursor += 1;
                    }
                    redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                    surface::present_rect(info.surface_cap, current_line_y, 16);
                }
            }
            let t_after_present = rdtsc();
            // Real, disclosed logging note: `ascii=` is kept (not
            // renamed to `scancode=`) for compatibility with
            // `scripts/test-terminal.ps1`'s existing real evidence
            // checks -- Left/Right have no real ASCII value, so they
            // log a real, fixed, out-of-band sentinel (not a genuine
            // ASCII code) instead of a fabricated one.
            let logged_ascii: u64 = match key {
                Key::Ascii(ch) => ch as u64,
                Key::Enter => b'\n' as u64,
                Key::Backspace => 0x08,
                Key::Left => 0x11, // real, fixed sentinel -- not a genuine ASCII code
                Key::Right => 0x12, // real, fixed sentinel -- not a genuine ASCII code
            };
            com1::write_str("[TERMINAL_EMULATOR] KEY_ECHOED ascii=");
            com1::write_dec_u64(logged_ascii);
            com1::write_str("\n");
            com1::write_str("[TERMINAL_EMULATOR] LATENCY_CYCLES total=");
            com1::write_dec_u64(t_after_present.wrapping_sub(t_decode));
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
