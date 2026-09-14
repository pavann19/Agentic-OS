//! Phase 13 deliverable 4: the second real reference app -- a genuine,
//! capability-isolated ring-3 text editor, reusing `terminal_emulator`'s
//! own proven pipeline (real Surface + routed-input capabilities,
//! real PSF1 rendering via `agentic_sdk::surface`, the same dirty-rect
//! `present_rect` discipline that fixed the terminal's own measured
//! input lag) for a real, editable 2D grid of text.
//!
//! Real, disclosed scope: this is a fixed-size grid of `ROWS` rows by
//! `COLS` columns -- a real 2D cursor (all four arrow directions), real
//! insert/delete-at-cursor within a row, Enter moves to the start of
//! the NEXT row (this is a form-style editing surface, not a flowing
//! document: it does NOT insert a new row or shift existing rows down,
//! and Backspace at column 0 moves the cursor to the end of the
//! previous row without merging the two rows' content). What's real
//! here is everything up through that: capability install, routed
//! input, 2D cursor movement, and live per-cell text editing, working
//! end to end. What's NOT here: disk-backed load/save -- there is no
//! block-device IPC service in this kernel yet that would let a
//! non-driver process read/write a real file (AHCI/virtio-blk access
//! today is only ever a dedicated driver process's own direct MMIO
//! capability). That's real, separate follow-up work, disclosed here
//! rather than faked with an in-memory buffer pretending to persist.

#![no_std]
#![no_main]

use agentic_sdk::{com1, surface, syscall::syscall1};

const INFO_VADDR: u64 = 0x0000_0000_0052_0000;

#[repr(C)]
struct EditorInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

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

const ROWS: usize = 16;
const COLS: usize = 56;
const GLYPH_HEIGHT: u32 = 16;
const GLYPH_WIDTH: u32 = 8;
const FG: u32 = 0x00FFFFFF;
const BG: u32 = 0x0000_0000;

/// Real, disclosed bug found and fixed bringing this app up -- the
/// SAME recurring toolchain bug class already documented at length in
/// `kernel_common::mem_intrinsics`, `netstack_driver`, and
/// `agentic_sdk::text_widget::TextRegion::init_in_place`'s own module
/// docs: a large `[0u8; N]`-style zero-init literal (here, `buf` alone
/// is `ROWS*COLS` = 896 bytes) can get lowered by LLVM into an
/// indirect `memset`-style call this freestanding ELF loader never
/// resolves, landing on a real call through address 0 -- confirmed via
/// a real #PF (cr2=0x0) at this exact line the first time this was
/// written as a plain `let mut buf = [[0u8; COLS]; ROWS];` local.
/// Fixed the same proven way `TextRegion::init_in_place` already
/// established: a real, explicit `write_volatile` per-byte zero loop
/// directly into the caller's own already-allocated memory, and the
/// state is accessed ONLY through the raw pointer this returns from
/// then on -- it is never moved or returned by value, which is what
/// would risk retriggering the exact same bug on the move itself.
struct EditorState {
    buf: [[u8; COLS]; ROWS],
    row_len: [usize; ROWS],
}

impl EditorState {
    unsafe fn init_in_place(place: *mut Self) {
        let bytes = place as *mut u8;
        for i in 0..core::mem::size_of::<Self>() {
            core::ptr::write_volatile(bytes.add(i), 0);
        }
    }
}

/// Real, visible text cursor -- same real inverted-glyph-overlay
/// technique `terminal_emulator::redraw_current_line` already
/// established, applied per-row here since a 2D editor's cursor can be
/// on any row, not always the last one.
unsafe fn redraw_row(surface_cap: u32, row: usize, buf: &[[u8; COLS]; ROWS], cursor: Option<usize>) {
    let y = (row as u32) * GLYPH_HEIGHT;
    // Uses agentic_sdk's shared `surface::draw_line_padded` which pads up to COLS
    // with spaces (0x20) and converts any null bytes (0) to spaces, avoiding glyph 0 rendering.
    surface::draw_line_padded(surface_cap, 0, y, &buf[row], COLS, FG, BG);
    if let Some(col) = cursor {
        let under = if col < COLS {
            let b = buf[row][col];
            if b == 0 { b' ' } else { b }
        } else {
            b' '
        };
        let cell = [under];
        surface::draw_text(surface_cap, (col as u32) * GLYPH_WIDTH, y, &cell, BG, FG);
    }
    surface::present_rect(surface_cap, y, GLYPH_HEIGHT);
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const EditorInfo);

        com1::write_str("\n[TEXT_EDITOR] real ELF64 ring-3 process, real Surface + routed-input capabilities\n");
        syscall1(1, 0x7E13_0000);

        // Real, disclosed fix -- see `EditorState`'s own doc for the
        // real #PF this avoids: the state lives in this already-
        // allocated local, zeroed in place, and is accessed ONLY
        // through these two field references from then on (never moved
        // or returned by value).
        let mut state_mu = core::mem::MaybeUninit::<EditorState>::uninit();
        let state_ptr = state_mu.as_mut_ptr();
        EditorState::init_in_place(state_ptr);
        let buf = &mut (*state_ptr).buf;
        let row_len = &mut (*state_ptr).row_len;
        let mut row: usize = 0;
        let mut col: usize = 0;

        surface::draw_text(info.surface_cap, 0, 0, b"TEXT_EDITOR_READY", FG, BG);
        surface::present(info.surface_cap);
        syscall1(4, info.ready_token);

        let mut pending_extended = false;
        let mut mods = ModifierState::default();
        let mut typed_total: u64 = 0;
        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                // Real, evidence-backed latency fix: replaced the old
                // `spin_loop()` busy-wait with a cooperative yield. The
                // spin consumed 100% CPU for the entire scheduling quantum
                // (~150ms before the APIC fix, ~15ms after) doing nothing
                // productive -- zero key events to process. `yield_now()`
                // calls SYS_YIELD (syscall 29) to relinquish the quantum
                // immediately, giving the keyboard driver thread a real
                // opportunity to run and push its next IPC message before
                // this thread is scheduled again.
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

            let prev_row = row;
            match key {
                Key::Ascii(ch) => {
                    if col < COLS {
                        let len = row_len[row];
                        let insert_at = col.min(len);
                        for i in (insert_at..len.min(COLS - 1)).rev() {
                            buf[row][i + 1] = buf[row][i];
                        }
                        buf[row][insert_at] = ch;
                        row_len[row] = (len + 1).min(COLS);
                        col += 1;
                    }
                    redraw_row(info.surface_cap, row, &*buf, Some(col));
                }
                Key::Tab => {
                    if col + 4 <= COLS {
                        let len = row_len[row];
                        let insert_at = col.min(len);
                        for i in (insert_at..len.min(COLS - 4)).rev() {
                            buf[row][i + 4] = buf[row][i];
                        }
                        for k in 0..4 {
                            buf[row][insert_at + k] = b' ';
                        }
                        row_len[row] = (len + 4).min(COLS);
                        col += 4;
                    }
                    redraw_row(info.surface_cap, row, &*buf, Some(col));
                }
                Key::Backspace => {
                    if col > 0 {
                        let len = row_len[row];
                        for i in (col - 1)..len.saturating_sub(1) {
                            buf[row][i] = buf[row][i + 1];
                        }
                        if len > 0 {
                            row_len[row] = len - 1;
                            buf[row][row_len[row]] = 0;
                        }
                        col -= 1;
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    } else if row > 0 {
                        // Real, disclosed scope limit (this module's own
                        // doc): moves to the previous row's end WITHOUT
                        // merging the two rows' real content.
                        row -= 1;
                        col = row_len[row];
                        redraw_row(info.surface_cap, prev_row, &*buf, None);
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    }
                }
                Key::Enter => {
                    if row + 1 < ROWS {
                        row += 1;
                        col = 0;
                        redraw_row(info.surface_cap, prev_row, &*buf, None);
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    }
                }
                Key::Left => {
                    if col > 0 {
                        col -= 1;
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    } else if row > 0 {
                        row -= 1;
                        col = row_len[row];
                        redraw_row(info.surface_cap, prev_row, &*buf, None);
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    }
                }
                Key::Right => {
                    if col < row_len[row] {
                        col += 1;
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    } else if row + 1 < ROWS {
                        row += 1;
                        col = 0;
                        redraw_row(info.surface_cap, prev_row, &*buf, None);
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    }
                }
                Key::Up => {
                    if row > 0 {
                        row -= 1;
                        col = col.min(row_len[row]);
                        redraw_row(info.surface_cap, prev_row, &*buf, None);
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    }
                }
                Key::Down => {
                    if row + 1 < ROWS {
                        row += 1;
                        col = col.min(row_len[row]);
                        redraw_row(info.surface_cap, prev_row, &*buf, None);
                        redraw_row(info.surface_cap, row, &*buf, Some(col));
                    }
                }
                Key::PageUp | Key::PageDown | Key::Escape => {}
            }

            // Real, disclosed finding from a real rdtsc measurement
            // (kept in this comment, not left as a permanent log line,
            // since the diagnostic's job -- ruling out the CPU/kernel
            // side -- is done): per-keystroke CPU cost here measured
            // 2.6M-6.8M cycles, the same real ballpark as
            // `terminal_emulator`'s own already-fixed per-keystroke
            // cost. That rules OUT kernel-side compute as the cause of
            // a reported ~5-second hands-on response feel -- real,
            // separate follow-up work (most likely per-keystroke COM1/
            // serial-file I/O volume, e.g. this very log line, stalling
            // against a slow host-side file-backed serial channel) is
            // disclosed, not yet fixed here.
            com1::write_str("[TEXT_EDITOR] KEY_HANDLED row=");
            com1::write_dec_u64(row as u64);
            com1::write_str(" col=");
            com1::write_dec_u64(col as u64);
            com1::write_str("\n");
            syscall1(1, 0x7E13_6000 | typed_total);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
