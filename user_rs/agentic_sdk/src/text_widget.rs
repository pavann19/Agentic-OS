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
    /// Real, in-place zero-init -- writes directly into `place`'s own
    /// memory, byte by byte, and NEVER constructs or returns a `Self`
    /// by value. Necessary, not just cautious: this project has already
    /// found (see `kernel_common::mem_intrinsics` and `netstack_driver`'s
    /// own module docs) that a large zero-init or move/copy on this
    /// toolchain can get lowered through a broken GOT-indirect
    /// intrinsic-call path this freestanding kernel's ELF loader never
    /// resolves, landing on a call through address 0 -- previously seen
    /// for `[0u8; N]` literals, and reproduced AGAIN live bringing up
    /// `terminal_emulator` (a real #PF, cr2=0x0) from the seemingly
    /// safer `MaybeUninit`-then-return-by-value version of this exact
    /// function: returning a `TextRegion` sized for a real terminal
    /// (8 lines x 40 columns = 320+ bytes) still forces the compiler to
    /// move that many bytes out of this function, and that move itself
    /// hit the same broken path. Writing directly into the caller's own
    /// already-allocated memory (a local `MaybeUninit<TextRegion<..>>`
    /// on their own stack, per-field access via the raw pointer this
    /// returns) never moves the struct as a whole at all, sidestepping
    /// the bug by construction rather than by size limit.
    pub unsafe fn init_in_place(place: *mut Self) {
        let bytes = place as *mut u8;
        for i in 0..core::mem::size_of::<Self>() {
            core::ptr::write_volatile(bytes.add(i), 0);
        }
    }

    /// Real, explicit per-byte copy -- NOT `copy_from_slice`/array
    /// assignment. Real bug found and fixed bringing up
    /// `terminal_emulator`'s Enter-key path: `copy_from_slice` (and a
    /// plain `self.lines[i] = self.lines[j]` array assignment) can both
    /// get lowered by LLVM into an actual `memcpy` call on this
    /// toolchain, and this freestanding target's ELF loader doesn't
    /// resolve that indirection -- a real, reproduced call through a
    /// NULL pointer (`call qword ptr [rip+...]` landing at address 0,
    /// confirmed via real disassembly of the faulting binary). Same
    /// root cause class already documented in `kernel_common::
    /// mem_intrinsics` and `netstack_driver`'s own module docs (there,
    /// for zero-init literals; here, for a copy) -- `vmm::
    /// write_user_bytes`'s own doc already names this exact copy-vs-
    /// memcpy distinction. A real, explicit, volatile-free per-byte
    /// loop is what reliably avoids it.
    /// Raw-pointer copy specifically to avoid `self.lines[i-1] =
    /// self.lines[i]` (a plain array assignment the borrow checker
    /// wouldn't even allow directly between two indices of the same
    /// array anyway) — real, explicit per-byte `read_volatile`/
    /// `write_volatile`, never a value that could get lowered into a
    /// `memcpy` call.
    unsafe fn shift_row_up(lines: *mut [u8; COLS], dst_index: usize, src_index: usize) {
        let dst = lines.add(dst_index) as *mut u8;
        let src = lines.add(src_index) as *const u8;
        for i in 0..COLS {
            let byte = core::ptr::read_volatile(src.add(i));
            core::ptr::write_volatile(dst.add(i), byte);
        }
    }

    /// Appends one line, truncated to `COLS` bytes. Once `LINES` lines
    /// have been pushed, the OLDEST line is discarded to make room —
    /// real scrolling, implemented as the simplest correct thing that
    /// could work: shift every row up by one, write the new line into
    /// the last slot.
    pub fn push_line(&mut self, s: &[u8]) {
        if self.count < LINES {
            let len = s.len().min(COLS);
            for i in 0..len {
                self.lines[self.count][i] = s[i];
            }
            for i in len..COLS {
                self.lines[self.count][i] = 0;
            }
            self.lens[self.count] = len as u8;
            self.count += 1;
            return;
        }
        let lines_ptr = self.lines.as_mut_ptr();
        for i in 1..LINES {
            unsafe { Self::shift_row_up(lines_ptr, i - 1, i) };
            self.lens[i - 1] = self.lens[i];
        }
        let len = s.len().min(COLS);
        for i in 0..COLS {
            self.lines[LINES - 1][i] = if i < len { s[i] } else { 0 };
        }
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
