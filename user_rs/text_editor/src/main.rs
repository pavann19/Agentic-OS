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
}

/// Same real PS/2 Set 1 decode discipline as `terminal_emulator`'s own
/// `decode_key` (see its doc) -- extended here to also decode Up/Down,
/// which a multi-row editor actually needs (the terminal's own single
/// editable line never did).
fn decode_key(code: u8, extended: bool) -> Option<Key> {
    if code & 0x80 != 0 {
        return None; // break code
    }
    if extended {
        return match code {
            0x4B => Some(Key::Left),
            0x4D => Some(Key::Right),
            0x48 => Some(Key::Up),
            0x50 => Some(Key::Down),
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
    surface::draw_text(surface_cap, 0, y, &buf[row], FG, BG);
    if let Some(col) = cursor {
        let under = if col < COLS { buf[row][col] } else { b' ' };
        let cell = [if under == 0 { b' ' } else { under }];
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
        let mut typed_total: u64 = 0;
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
            }

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
