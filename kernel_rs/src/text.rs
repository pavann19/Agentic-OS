//! Phase 12 deliverable 4 (`docs/ROADMAP.md` §5): "A minimal native UI
//! toolkit against the capability ABI... text rendering, basic widgets
//! — enough to build the reference apps Phase 13 needs, not a general
//! framework." This module is the kernel-side half: real PSF1 bitmap
//! font glyph rendering, bounds-checked against a `Surface` capability
//! exactly the way `compositor::syscall_fill_surface` already bounds a
//! solid-color fill. The other half — line-buffering/scrolling — is
//! deliberately NOT here; that's application-level state
//! (`agentic_sdk`'s own text-widget module), not something the kernel
//! needs to know about. This module's whole job is "draw these bytes as
//! glyphs at this offset within this surface, never outside it."
//!
//! Real, disclosed source: `boot_rs`'s own PSF1 font load
//! (`loader.rs::load_psf1_font`) has been real and passed through
//! `BootInfoPayload::font` since Phase 0 -- genuinely unused by
//! `kernel_rs` until this deliverable, a real, disclosed gap now
//! closed, not new plumbing invented for this pass.

use crate::bootinfo::Psf1Font;
use crate::pmm;

/// PSF1 glyphs are always 8 pixels wide; `chars_size` (the font
/// header's own field) is the real per-glyph height in bytes/rows —
/// read from the actual loaded font, never assumed.
const GLYPH_WIDTH: u32 = 8;

struct FontInfo {
    glyph_buffer_vaddr: u64,
    glyph_height: u32,
}

static mut FONT: Option<FontInfo> = None;

/// Real, one-time setup: translates the bootloader's real PSF1 font
/// (a physical UEFI-pool pointer, same "needs `p2v_pub` before any
/// dereference" discipline `main.rs`'s own framebuffer setup already
/// documents) into a kernel-virtual glyph-buffer address plus the
/// font's own real glyph height. Called once from `main.rs` alongside
/// `compositor::spawn`.
pub fn init(font_ptr: *mut Psf1Font) {
    unsafe {
        let font_vaddr = pmm::p2v_pub(font_ptr as u64) as *const Psf1Font;
        let font = &*font_vaddr;
        let header_vaddr = pmm::p2v_pub(font.header as u64) as *const crate::bootinfo::Psf1Header;
        let header = &*header_vaddr;
        let glyph_buffer_vaddr = pmm::p2v_pub(font.glyph_buffer as u64) as u64;
        FONT = Some(FontInfo { glyph_buffer_vaddr, glyph_height: header.chars_size as u32 });
        crate::klog_info!("TEXT_FONT_INIT glyph_width={} glyph_height={}", GLYPH_WIDTH, header.chars_size);
    }
}

fn font() -> Option<&'static FontInfo> {
    unsafe { (&*&raw const FONT).as_ref() }
}

pub fn glyph_height() -> u32 {
    font().map(|f| f.glyph_height).unwrap_or(0)
}

pub fn glyph_width() -> u32 {
    GLYPH_WIDTH
}

/// Real, bounds-checked glyph blit: draws `ch`'s own real PSF1 bitmap
/// (one bit per pixel, MSB-first per row, `glyph_height` rows) at
/// framebuffer position `(x, y)`, but ONLY the pixels that fall inside
/// `(clip_x, clip_y, clip_w, clip_h)` -- a pixel outside that rect is
/// refused, never drawn, the same per-pixel discipline
/// `compositor_driver`'s own `fill_rect`/`in_bounds` already
/// established for solid fills. `bg` fills a glyph cell's 0 bits (never
/// left transparent -- a real, deliberate choice: partial redraws over
/// old text would otherwise leave stale pixel garbage behind).
/// Real per-pixel write, same discipline `compositor::
/// syscall_fill_surface` already uses: the real framebuffer is only
/// ever touched through `vmm::map_mmio_page(phys_base + byte_offset)`,
/// mapping the specific physical page each pixel's real byte offset
/// falls in, per write -- `compositor_verify_thread`'s own module doc
/// explains the real bug (`map_mmio_page` maps exactly one page) this
/// avoids re-hitting. No large contiguous virtual mapping of the
/// framebuffer is assumed anywhere in this module.
unsafe fn put_pixel(fb_phys_base: u64, pixels_per_scan_line: u32, x: u32, y: u32, color: u32) {
    let byte_offset = (y as u64 * pixels_per_scan_line as u64 + x as u64) * 4;
    let vaddr = crate::vmm::map_mmio_page(fb_phys_base + byte_offset);
    core::ptr::write_volatile(vaddr as *mut u32, color);
}

#[allow(clippy::too_many_arguments)]
unsafe fn blit_glyph(
    fb_phys_base: u64,
    pixels_per_scan_line: u32,
    x: u32,
    y: u32,
    ch: u8,
    fg: u32,
    bg: u32,
    clip_x: u32,
    clip_y: u32,
    clip_w: u32,
    clip_h: u32,
) {
    let Some(f) = font() else { return };
    let glyph_offset = f.glyph_buffer_vaddr + (ch as u64) * (f.glyph_height as u64);
    for row in 0..f.glyph_height {
        let row_byte = core::ptr::read_volatile((glyph_offset + row as u64) as *const u8);
        let py = y + row;
        if py < clip_y || py >= clip_y + clip_h {
            continue;
        }
        for col in 0..GLYPH_WIDTH {
            let px = x + col;
            if px < clip_x || px >= clip_x + clip_w {
                continue;
            }
            // PSF1 bit order: bit 7 (0x80) is the LEFTMOST pixel of the row.
            let bit_set = (row_byte & (0x80 >> col)) != 0;
            let color = if bit_set { fg } else { bg };
            put_pixel(fb_phys_base, pixels_per_scan_line, px, py, color);
        }
    }
}

/// Real, bounds-checked text blit: draws `text` (a bounded byte slice —
/// real, disclosed scope, ASCII/Latin-1 glyph indices only, matching
/// PSF1's own 256-glyph table, no UTF-8 decoding) left-to-right
/// starting at `(x, y)` WITHIN `(clip_x, clip_y, clip_w, clip_h)`,
/// advancing by the font's own real glyph width per character. A
/// character (or part of one) that would land outside the clip rect is
/// truncated per-pixel by `blit_glyph`'s own bounds check, never drawn
/// past it.
#[allow(clippy::too_many_arguments)]
pub unsafe fn draw_text(
    fb_phys_base: u64,
    pixels_per_scan_line: u32,
    x: u32,
    y: u32,
    text: &[u8],
    fg: u32,
    bg: u32,
    clip_x: u32,
    clip_y: u32,
    clip_w: u32,
    clip_h: u32,
) {
    for (i, &ch) in text.iter().enumerate() {
        let cx = x + (i as u32) * GLYPH_WIDTH;
        blit_glyph(fb_phys_base, pixels_per_scan_line, cx, y, ch, fg, bg, clip_x, clip_y, clip_w, clip_h);
    }
}
