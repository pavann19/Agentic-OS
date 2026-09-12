//! Phase 12 deliverable 4: the ring-3 side of `SYS_SURFACE_DRAW_TEXT`
//! (syscall 14, `kernel_rs::compositor::syscall_draw_text`) — real PSF1
//! text rendering into a held `Surface` capability, bounds-checked by
//! the kernel exactly like `SYS_SURFACE_FILL` (syscall 11) already is.

use crate::syscall::syscall2;

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
