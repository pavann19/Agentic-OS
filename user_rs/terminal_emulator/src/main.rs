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

/// Real, decoded key events this terminal reacts to.
#[derive(Clone, Copy)]
enum Key {
    Ascii(u8),
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Escape,
    Tab,
}

#[derive(Default)]
struct ModifierState {
    shift: bool,
    caps_lock: bool,
    ctrl: bool,
    alt: bool,
}

/// Real, bounded PS/2 Set 1 decode with Shift/Caps/Ctrl modifier tracking
/// and expanded ASCII symbols/punctuation.
fn decode_key(code: u8, extended: bool, mods: &mut ModifierState) -> Option<Key> {
    // Modifier break codes:
    if code == 0xAA || code == 0xB6 {
        mods.shift = false;
        return None;
    }
    if code == 0x9D {
        mods.ctrl = false;
        return None;
    }
    if code == 0xB8 {
        mods.alt = false;
        return None;
    }

    // Any other break code is ignored
    if code & 0x80 != 0 {
        return None;
    }

    // Modifier make codes:
    if code == 0x2A || code == 0x36 {
        mods.shift = true;
        return None;
    }
    if code == 0x1D {
        mods.ctrl = true;
        return None;
    }
    if code == 0x38 {
        mods.alt = true;
        return None;
    }
    if code == 0x3A {
        mods.caps_lock = !mods.caps_lock;
        return None;
    }

    if extended {
        return match code {
            0x4B => Some(Key::Left),
            0x4D => Some(Key::Right),
            0x48 => Some(Key::Up),
            0x50 => Some(Key::Down),
            0x49 => Some(Key::PageUp),
            0x51 => Some(Key::PageDown),
            _ => None,
        };
    }

    match code {
        0x01 => Some(Key::Escape),
        0x0E => Some(Key::Backspace),
        0x0F => Some(Key::Tab),
        0x1C => Some(Key::Enter),
        0x39 => Some(Key::Ascii(b' ')),

        // Letters 'a'..'z' (uppercase if shift ^ caps_lock)
        0x1E => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'A' } else { b'a' })),
        0x30 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'B' } else { b'b' })),
        0x2E => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'C' } else { b'c' })),
        0x20 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'D' } else { b'd' })),
        0x12 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'E' } else { b'e' })),
        0x21 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'F' } else { b'f' })),
        0x22 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'G' } else { b'g' })),
        0x23 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'H' } else { b'h' })),
        0x17 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'I' } else { b'i' })),
        0x24 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'J' } else { b'j' })),
        0x25 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'K' } else { b'k' })),
        0x26 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'L' } else { b'l' })),
        0x32 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'M' } else { b'm' })),
        0x31 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'N' } else { b'n' })),
        0x18 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'O' } else { b'o' })),
        0x19 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'P' } else { b'p' })),
        0x10 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'Q' } else { b'q' })),
        0x13 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'R' } else { b'r' })),
        0x1F => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'S' } else { b's' })),
        0x14 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'T' } else { b't' })),
        0x16 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'U' } else { b'u' })),
        0x2F => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'V' } else { b'v' })),
        0x11 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'W' } else { b'w' })),
        0x2D => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'X' } else { b'x' })),
        0x15 => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'Y' } else { b'y' })),
        0x2C => Some(Key::Ascii(if mods.shift ^ mods.caps_lock { b'Z' } else { b'z' })),

        // Number row: 1..0 and symbols
        0x02 => Some(Key::Ascii(if mods.shift { b'!' } else { b'1' })),
        0x03 => Some(Key::Ascii(if mods.shift { b'@' } else { b'2' })),
        0x04 => Some(Key::Ascii(if mods.shift { b'#' } else { b'3' })),
        0x05 => Some(Key::Ascii(if mods.shift { b'$' } else { b'4' })),
        0x06 => Some(Key::Ascii(if mods.shift { b'%' } else { b'5' })),
        0x07 => Some(Key::Ascii(if mods.shift { b'^' } else { b'6' })),
        0x08 => Some(Key::Ascii(if mods.shift { b'&' } else { b'7' })),
        0x09 => Some(Key::Ascii(if mods.shift { b'*' } else { b'8' })),
        0x0A => Some(Key::Ascii(if mods.shift { b'(' } else { b'9' })),
        0x0B => Some(Key::Ascii(if mods.shift { b')' } else { b'0' })),

        // Punctuation and symbols
        0x0C => Some(Key::Ascii(if mods.shift { b'_' } else { b'-' })),
        0x0D => Some(Key::Ascii(if mods.shift { b'+' } else { b'=' })),
        0x1A => Some(Key::Ascii(if mods.shift { b'{' } else { b'[' })),
        0x1B => Some(Key::Ascii(if mods.shift { b'}' } else { b']' })),
        0x27 => Some(Key::Ascii(if mods.shift { b':' } else { b';' })),
        0x28 => Some(Key::Ascii(if mods.shift { b'"' } else { b'\'' })),
        0x29 => Some(Key::Ascii(if mods.shift { b'~' } else { b'`' })),
        0x2B => Some(Key::Ascii(if mods.shift { b'|' } else { b'\\' })),
        0x33 => Some(Key::Ascii(if mods.shift { b'<' } else { b',' })),
        0x34 => Some(Key::Ascii(if mods.shift { b'>' } else { b'.' })),
        0x35 => Some(Key::Ascii(if mods.shift { b'?' } else { b'/' })),

        _ => None,
    }
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
    surface::draw_line_padded(surface_cap, 0, y, &current[..current_len], COLS, FG, BG);
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
        // Real bug found and fixed: unused trailing cells used to be
        // zero-filled (0x00), but this font's glyph 0 is NOT blank --
        // draw_text always renders the FULL COLS-width `current` array
        // every redraw (by design, so a shrinking line's old trailing
        // glyphs get overwritten rather than left stale), so every
        // never-yet-typed cell was visibly rendering as glyph 0's real
        // symbol instead of blank space. Space (0x20) is what a real
        // "nothing typed here" cell should render as.
        let mut current: [u8; COLS] = [b' '; COLS];
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
        let mut mods = ModifierState::default();
        let current_line_y = (LINES as u32) * 16;
        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                // Cooperative yield: same fix as text_editor -- avoid
                // burning a full scheduling quantum when the IPC key queue
                // is empty. SYS_YIELD (syscall 29) relinquishes immediately.
                surface::yield_now();
                continue;
            }
            let scancode = r as u8;
            if scancode == 0xE0 {
                pending_extended = true;
                continue;
            }
            let extended = pending_extended;
            pending_extended = false;
            let Some(key) = decode_key(scancode, extended, &mut mods) else {
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
                    current = [b' '; COLS]; // see the initial declaration's own doc comment for why space, not 0
                    current_len = 0;
                    cursor = 0;
                    (*history_ptr).render(info.surface_cap, 16, FG, BG);
                    redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                    surface::present(info.surface_cap);
                }
                Key::Backspace => {
                    cursor = cursor.min(current_len);
                    if cursor > 0 && current_len > 0 {
                        for i in (cursor - 1)..(current_len - 1) {
                            current[i] = current[i + 1];
                        }
                        current_len -= 1;
                        current[current_len] = b' ';
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
                Key::Tab => {
                    cursor = cursor.min(current_len);
                    if current_len + 4 <= COLS {
                        for i in (cursor..current_len).rev() {
                            current[i + 4] = current[i];
                        }
                        for k in 0..4 {
                            current[cursor + k] = b' ';
                        }
                        current_len += 4;
                        cursor += 4;
                        redraw_current_line(info.surface_cap, current_line_y, &current, current_len, cursor);
                        surface::present_rect(info.surface_cap, current_line_y, 16);
                    }
                }
                Key::Ascii(ch) => {
                    cursor = cursor.min(current_len);
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
                Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Escape => {}
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
                Key::Left => 0x11,
                Key::Right => 0x12,
                Key::Up => 0x13,
                Key::Down => 0x14,
                Key::PageUp => 0x15,
                Key::PageDown => 0x16,
                Key::Escape => 0x1B,
                Key::Tab => 0x09,
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
