//! Phase 12 deliverable 4: the ring-3 side of `SYS_SURFACE_DRAW_TEXT`
//! (syscall 14, `kernel_rs::compositor::syscall_draw_text`) — real PSF1
//! text rendering into a held `Surface` capability, bounds-checked by
//! the kernel exactly like `SYS_SURFACE_FILL` (syscall 11) already is.

use crate::syscall::{syscall1, syscall2};

/// MUST stay field-for-field identical to `kernel_rs::compositor::
/// SurfaceTextRequest` — this is a raw, unchecked-by-the-compiler ABI
/// contract between the two crates (the same shape every driver's own
/// `INFO_VADDR`-mapped info struct already relies on), not something
/// either side can drift on its own without silently breaking the
/// other.
#[repr(C)]
struct SurfaceTextRequest {
    x: u32,
    y: u32,
    fg: u32,
    bg: u32,
    text_vaddr: u64,
    text_len: u32,
}

/// Real, disclosed bound matching the kernel's own `MAX_DRAW_TEXT_LEN`
/// — a request naming a longer `text` is truncated, never rejected
/// outright (matches `text::draw_text`'s own per-pixel clip philosophy:
/// draw as much as legitimately fits, refuse only what doesn't).
const MAX_DRAW_TEXT_LEN: usize = 256;

/// Draws `text` (ASCII/Latin-1 byte values — PSF1's own glyph-index
/// space, no UTF-8 decoding) at `(x, y)` relative to the surface named
/// by `surface_cap` (a `CapId` this process already holds, e.g. the
/// same one `SYS_SURFACE_FILL` already uses). Returns `0` on success,
/// `u64::MAX` if the capability doesn't resolve to a real `Surface` the
/// caller actually holds.
pub unsafe fn draw_text(surface_cap: u32, x: u32, y: u32, text: &[u8], fg: u32, bg: u32) -> u64 {
    let len = text.len().min(MAX_DRAW_TEXT_LEN);
    let req = SurfaceTextRequest {
        x,
        y,
        fg,
        bg,
        text_vaddr: text.as_ptr() as u64,
        text_len: len as u32,
    };
    let req_vaddr = &req as *const SurfaceTextRequest as u64;
    syscall2(14, surface_cap as u64, req_vaddr)
}

/// Real, disclosed latency fix: `SYS_SURFACE_FILL`/`SYS_SURFACE_DRAW_TEXT`
/// only write into this window's own in-memory buffer now — nothing
/// reaches the real, on-screen framebuffer until this is called. A
/// caller should do a whole batch of fill/draw_text calls, then call
/// this exactly ONCE (syscall 16, `SYS_SURFACE_PRESENT`) to make that
/// batch visible in one real recomposite, not one per call.
pub unsafe fn present(surface_cap: u32) -> u64 {
    syscall1(16, surface_cap as u64)
}

/// Real, disclosed partial-present fast path (see `kernel_rs::
/// window_manager::present_partial`'s own doc): presents ONLY local
/// rows `[y, y+height)` of this window's content -- no title bar, no
/// other rows. Real, disclosed caller obligation: only correct when
/// the caller KNOWS nothing else about this window (or any other
/// window) changed since the last present -- e.g. a terminal that just
/// updated one text line without scrolling. A caller unsure should use
/// `present` instead.
pub unsafe fn present_rect(surface_cap: u32, y: u32, height: u32) -> u64 {
    let packed = ((y as u64) << 32) | (height.max(1) as u64);
    syscall2(16, surface_cap as u64, packed)
}

/// Draws `text` at `(x, y)` padded with spaces (`b' '`) up to `total_cols`.
/// This prevents the recurring visual bug where unwritten or trailing cells
/// in a fixed-width line render as PSF1 glyph 0 (which is not blank in PSF fonts).
///
/// Embedded null bytes (0) in `text` are also safely converted to spaces (`b' '`).
pub unsafe fn draw_line_padded(
    surface_cap: u32,
    x: u32,
    y: u32,
    text: &[u8],
    total_cols: usize,
    fg: u32,
    bg: u32,
) -> u64 {
    let total = total_cols.min(MAX_DRAW_TEXT_LEN);
    let mut buf_mu = core::mem::MaybeUninit::<[u8; MAX_DRAW_TEXT_LEN]>::uninit();
    let buf_ptr = buf_mu.as_mut_ptr() as *mut u8;
    let copy_len = text.len().min(total);
    for i in 0..copy_len {
        let b = text[i];
        core::ptr::write_volatile(buf_ptr.add(i), if b == 0 { b' ' } else { b });
    }
    for i in copy_len..total {
        core::ptr::write_volatile(buf_ptr.add(i), b' ');
    }
    let slice = core::slice::from_raw_parts(buf_ptr, total);
    draw_text(surface_cap, x, y, slice, fg, bg)
}

#[repr(C)]
pub struct SurfaceBitmapRequest {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub fg: u32,
    pub bg: u32,
    pub data_vaddr: u64,
    pub data_len: u32,
}

/// Draws a 1-bit monochrome bitmap/icon into the Surface named by `surface_cap`.
/// 1-bit = `fg` color. 0-bit = `bg` color (or transparent/skipped if `bg == 0`).
pub unsafe fn draw_bitmap(
    surface_cap: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    data: &[u8],
    fg: u32,
    bg: u32,
) -> u64 {
    let req = SurfaceBitmapRequest {
        x,
        y,
        width,
        height,
        fg,
        bg,
        data_vaddr: data.as_ptr() as u64,
        data_len: data.len() as u32,
    };
    let req_vaddr = &req as *const SurfaceBitmapRequest as u64;
    syscall2(23, surface_cap as u64, req_vaddr)
}
