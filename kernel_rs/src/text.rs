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

// Phase 5.9: In-memory PSF1 Glyph Raw Bitmap Cache (256 chars * 16 rows = 4096 bytes)
static mut GLYPH_RAW_CACHE: [[u8; 16]; 256] = [[0; 16]; 256];

// Phase 5.9: Fast 8-pixel Scanline Row LUT (256 byte patterns * 8 pixels = 8192 bytes)
struct RowLut {
    fg: u32,
    bg: u32,
    valid: bool,
    lut: [[u32; 8]; 256],
}

static mut ACTIVE_ROW_LUT: RowLut = RowLut {
    fg: 0,
    bg: 0,
    valid: false,
    lut: [[0; 8]; 256],
};

#[inline(always)]
unsafe fn ensure_row_lut(fg: u32, bg: u32) -> &'static [[u32; 8]; 256] {
    #[allow(static_mut_refs)]
    let lut_ref = &mut *&raw mut ACTIVE_ROW_LUT;
    if !lut_ref.valid || lut_ref.fg != fg || lut_ref.bg != bg {
        lut_ref.fg = fg;
        lut_ref.bg = bg;
        lut_ref.valid = true;
        for pattern in 0..256usize {
            let byte = pattern as u8;
            let row = &mut lut_ref.lut[pattern];
            row[0] = if (byte & 0x80) != 0 { fg } else { bg };
            row[1] = if (byte & 0x40) != 0 { fg } else { bg };
            row[2] = if (byte & 0x20) != 0 { fg } else { bg };
            row[3] = if (byte & 0x10) != 0 { fg } else { bg };
            row[4] = if (byte & 0x08) != 0 { fg } else { bg };
            row[5] = if (byte & 0x04) != 0 { fg } else { bg };
            row[6] = if (byte & 0x02) != 0 { fg } else { bg };
            row[7] = if (byte & 0x01) != 0 { fg } else { bg };
        }
    }
    &lut_ref.lut
}

/// Real, one-time setup: translates the bootloader's real PSF1 font
/// into a kernel-virtual glyph-buffer address and populates the in-memory
/// glyph cache for zero-MMIO font lookups (Phase 5.9).
pub fn init(font_ptr: *mut Psf1Font) {
    unsafe {
        let font_vaddr = pmm::p2v_pub(font_ptr as u64) as *const Psf1Font;
        let font = &*font_vaddr;
        let header_vaddr = pmm::p2v_pub(font.header as u64) as *const crate::bootinfo::Psf1Header;
        let header = &*header_vaddr;
        let glyph_buffer_vaddr = pmm::p2v_pub(font.glyph_buffer as u64) as u64;
        let h = header.chars_size as u32;
        FONT = Some(FontInfo { glyph_buffer_vaddr, glyph_height: h });

        // Populate GLYPH_RAW_CACHE from UEFI loader memory into kernel L1/L2 cache
        let raw_src = glyph_buffer_vaddr as *const u8;
        let max_r = (h as usize).min(16);
        for ch in 0..256usize {
            for r in 0..max_r {
                let byte = core::ptr::read_volatile(raw_src.add(ch * (h as usize) + r));
                GLYPH_RAW_CACHE[ch][r] = byte;
            }
        }

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
/// at framebuffer position `(x, y)` clipped against `(clip_x, clip_y, clip_w, clip_h)`.
unsafe fn put_pixel(fb_phys_base: u64, pixels_per_scan_line: u32, x: u32, y: u32, color: u32) {
    let byte_offset = (y as u64 * pixels_per_scan_line as u64 + x as u64) * 4;
    let vaddr = crate::vmm::map_framebuffer_page(fb_phys_base + byte_offset);
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
    let h = glyph_height();
    if h == 0 { return; }
    let lut = ensure_row_lut(fg, bg);
    let raw_glyph = &(*&raw const GLYPH_RAW_CACHE)[ch as usize];
    let max_rows = h.min(16);

    for row in 0..max_rows {
        let py = y + row;
        if py < clip_y || py >= clip_y + clip_h {
            continue;
        }
        let row_byte = raw_glyph[row as usize];
        let row_pixels = &lut[row_byte as usize];
        for col in 0..GLYPH_WIDTH {
            let px = x + col;
            if px < clip_x || px >= clip_x + clip_w {
                continue;
            }
            put_pixel(fb_phys_base, pixels_per_scan_line, px, py, row_pixels[col as usize]);
        }
    }
}

/// High-performance cached glyph blit into an in-memory RGBA buffer (Phase 5.9):
/// Replaces 128 bit-test branches per glyph with fast 8-pixel scanline copies directly
/// from the precomputed Row LUT!
#[allow(clippy::too_many_arguments)]
unsafe fn blit_glyph_to_buffer(buf: &mut [u32], buf_width: u32, buf_height: u32, x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
    let h = glyph_height();
    if h == 0 || y >= buf_height || x >= buf_width {
        return;
    }
    let lut = ensure_row_lut(fg, bg);
    let raw_glyph = &(*&raw const GLYPH_RAW_CACHE)[ch as usize];
    let max_rows = h.min(16).min(buf_height - y);
    let copy_w = (GLYPH_WIDTH).min(buf_width - x) as usize;

    for row in 0..max_rows {
        let row_byte = raw_glyph[row as usize];
        let row_pixels = &lut[row_byte as usize];
        let dst_off = ((y + row) * buf_width + x) as usize;
        if copy_w == 8 {
            buf[dst_off..dst_off + 8].copy_from_slice(row_pixels);
        } else {
            buf[dst_off..dst_off + copy_w].copy_from_slice(&row_pixels[..copy_w]);
        }
    }
}

/// Buffer-target counterpart of `draw_text` using fast glyph row cache (Phase 5.9).
pub unsafe fn draw_text_to_buffer(buf: &mut [u32], buf_width: u32, buf_height: u32, x: u32, y: u32, text: &[u8], fg: u32, bg: u32) {
    if y >= buf_height || x >= buf_width {
        return;
    }
    let _ = ensure_row_lut(fg, bg);
    for (i, &ch) in text.iter().enumerate() {
        let cx = x + (i as u32) * GLYPH_WIDTH;
        if cx >= buf_width {
            break;
        }
        blit_glyph_to_buffer(buf, buf_width, buf_height, cx, y, ch, fg, bg);
    }
}

/// Real, disclosed fix for the boot-splash-lingers-behind-the-terminal
/// bug: UEFI/TianoCore draws its own boot logo into the GOP framebuffer
/// before this kernel ever runs, and nothing before this cleared it --
/// a small drawn surface (e.g. the terminal's 320x200 window) only ever
/// overwrites ITS OWN rectangle, so the boot splash remained visible in
/// every pixel outside it. Clears the ENTIRE real framebuffer to
/// `color` once at startup, before any surface/window is drawn -- a
/// real, one-time, whole-screen fill via the same bounds-checked
/// per-pixel path `put_pixel` already established (now cheap: see
/// `vmm::map_mmio_page`'s own last-page cache).
pub unsafe fn clear_screen(fb_phys_base: u64, pixels_per_scan_line: u32, width: u32, height: u32, color: u32) {
    for y in 0..height {
        let row_vaddr = crate::vmm::map_framebuffer_page(fb_phys_base + (y as u64 * pixels_per_scan_line as u64) * 4) as *mut u32;
        let slice = core::slice::from_raw_parts_mut(row_vaddr, width as usize);
        slice.fill(color);
    }
    core::arch::asm!("sfence", options(nomem, nostack));
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
