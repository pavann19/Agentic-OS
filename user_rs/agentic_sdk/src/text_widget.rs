//! Phase 12 deliverable 4's other real half: "a minimal native UI
//! toolkit... enough to build the reference apps Phase 13 needs, not a
//! general framework." `surface::draw_text` (this crate) is the
//! kernel-mediated primitive; this module is the one real, minimal
//! WIDGET built on top of it — a fixed-capacity scrollable text
//! region, the single primitive Phase 13's own planned reference apps
//! (a terminal emulator, a text editor, a file manager's list view)
//! all reduce to: a bounded number of visible text lines that new
//! lines push older ones out of.
//!
//! Real, disclosed scope: no line-wrapping, no cursor, no editing —
//! `push_line`/`render` only. Scrolling STATE lives entirely here, in
//! application-level (ring-3) memory; the kernel has no notion of
//! "lines" or "scrolling" at all, only the one bounds-checked glyph
//! blit `surface::draw_text` already provides — matching this whole
//! toolkit's own stated goal of staying out of the kernel wherever
//! application-level state suffices.

use crate::surface;

/// `LINES` visible rows, each up to `COLS` bytes — both real, fixed,
/// compile-time bounds (no heap allocation, matching every other
/// `user_rs/*` crate's own no_std/no-allocator discipline).
pub struct TextRegion<const LINES: usize, const COLS: usize> {
    lines: [[u8; COLS]; LINES],
    lens: [u8; LINES],
    count: usize,
}

impl<const LINES: usize, const COLS: usize> TextRegion<LINES, COLS> {
    pub const fn new() -> Self {
        Self { lines: [[0u8; COLS]; LINES], lens: [0u8; LINES], count: 0 }
    }

    /// Appends one line, truncated to `COLS` bytes. Once `LINES` lines
    /// have been pushed, the OLDEST line is discarded to make room —
    /// real scrolling, implemented as the simplest correct thing that
    /// could work: shift every row up by one, write the new line into
    /// the last slot.
    pub fn push_line(&mut self, s: &[u8]) {
        if self.count < LINES {
            let len = s.len().min(COLS);
            self.lines[self.count][..len].copy_from_slice(&s[..len]);
            self.lens[self.count] = len as u8;
            self.count += 1;
            return;
        }
        for i in 1..LINES {
            self.lines[i - 1] = self.lines[i];
            self.lens[i - 1] = self.lens[i];
        }
        let len = s.len().min(COLS);
        self.lines[LINES - 1] = [0u8; COLS];
        self.lines[LINES - 1][..len].copy_from_slice(&s[..len]);
        self.lens[LINES - 1] = len as u8;
    }

    /// Draws every currently-held line into `surface_cap`, one real
    /// `SYS_SURFACE_DRAW_TEXT` call per line, top to bottom, each row
    /// `glyph_height` pixels below the last — the kernel's own bounds
    /// check (`compositor::syscall_draw_text`) truncates any row that
    /// would run past the surface's real edge, so a `TextRegion` sized
    /// larger than its target surface degrades to a real, visible
    /// partial render rather than corrupting anything outside it.
    pub unsafe fn render(&self, surface_cap: u32, glyph_height: u32, fg: u32, bg: u32) {
        for i in 0..self.count {
            let len = self.lens[i] as usize;
            surface::draw_text(surface_cap, 0, (i as u32) * glyph_height, &self.lines[i][..len], fg, bg);
        }
    }
}
