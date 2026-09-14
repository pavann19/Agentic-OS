//! Real window objects: replaces the direct "a Surface capability's
//! syscall writes land straight on the real framebuffer at a fixed,
//! baked-in position" model with an actual per-window OWN backing
//! buffer (plain RAM, `alloc::vec::Vec<u32>`) plus a real title bar,
//! composited onto the real framebuffer by `present()`.
//!
//! Accelerated double-buffered compositing architecture:
//! All compositing, window blitting, and damage tracking take place in
//! a dedicated RAM `BACKBUFFER` with contiguous memory operations
//! (`copy_from_slice`, `slice.fill`). Dirty scanlines are flushed to the
//! UEFI GOP framebuffer via fast scanline copies (`copy_nonoverlapping`),
//! eliminating MMIO traps / VM-exits in QEMU/WHPX.
//!
//! Cursor overlay is completely decoupled from window composition:
//! The cursor sprite is rendered only to the display layer without dirtying
//! the clean RAM backbuffer, allowing 60+ FPS mouse interaction with
//! zero latency and zero window recomposition overhead.

use crate::capability::ObjectId;
use crate::klog_info;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub const TITLE_BAR_HEIGHT: u32 = 20;
pub const TITLE_BAR_COLOR: u32 = 0x001E_293B; // Slate 800 - dark modern aesthetic
pub const TITLE_BAR_BORDER_COLOR: u32 = 0x0033_4155; // Slate 700
pub const TITLE_FG: u32 = 0x00F8_FAFC; // Crisp Slate 50
pub const MAX_TITLE_LEN: usize = 24;

// Real desktop chrome colors
pub const DESKTOP_BG_COLOR: u32 = 0x000F_172A; // Modern dark slate 900
pub const MENU_BAR_COLOR: u32 = 0x001E_293B; // Slate 800
pub const MENU_BAR_HEIGHT: u32 = 22; // 22px height fits 16px font + 3px padding
pub const MENU_BAR_BORDER_COLOR: u32 = 0x0033_4155; // 1px bottom border

// Real cursor state
static CURSOR_X: AtomicI32 = AtomicI32::new(160);
static CURSOR_Y: AtomicI32 = AtomicI32::new(100);
static LEFT_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);
static mut DRAGGING: Option<(ObjectId, i32, i32)> = None;

// RAM Backbuffer state for double-buffered compositing
static mut BACKBUFFER: Option<Vec<u32>> = None;
static mut BACKBUFFER_WIDTH: u32 = 0;
static mut BACKBUFFER_HEIGHT: u32 = 0;

pub struct Window {
    surface_object: ObjectId,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    buffer: Vec<u32>,
    title_buffer: Vec<u32>,
}

static mut WINDOWS: Option<Vec<Window>> = None;

#[allow(static_mut_refs)]
fn windows_mut() -> &'static mut Vec<Window> {
    unsafe {
        let slot = &mut *&raw mut WINDOWS;
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
        slot.as_mut().unwrap()
    }
}

#[allow(static_mut_refs)]
pub unsafe fn ensure_backbuffer(width: u32, height: u32) -> &'static mut [u32] {
    let slot = &mut *&raw mut BACKBUFFER;
    let size = (width * height) as usize;
    if slot.is_none() || BACKBUFFER_WIDTH != width || BACKBUFFER_HEIGHT != height {
        *slot = Some(vec![DESKTOP_BG_COLOR; size]);
        BACKBUFFER_WIDTH = width;
        BACKBUFFER_HEIGHT = height;
    }
    slot.as_mut().unwrap().as_mut_slice()
}

fn draw_circle_to_buffer(buf: &mut [u32], buf_width: u32, buf_height: u32, cx: i32, cy: i32, r: i32, color: u32) {
    let r2 = r * r;
    for dy in -r..=r {
        let py = cy + dy;
        if py < 0 || py >= buf_height as i32 {
            continue;
        }
        for dx in -r..=r {
            if dx * dx + dy * dy <= r2 {
                let px = cx + dx;
                if px >= 0 && px < buf_width as i32 {
                    buf[(py as u32 * buf_width + px as u32) as usize] = color;
                }
            }
        }
    }
}

/// Initializes desktop chrome into the RAM backbuffer and flushes to the GOP framebuffer
pub unsafe fn init_desktop_chrome(fb_phys_base: u64, ppsl: u32, width: u32, height: u32) {
    let bb = ensure_backbuffer(width, height);
    bb.fill(DESKTOP_BG_COLOR);

    // Top menu bar
    let menu_h = MENU_BAR_HEIGHT.min(height);
    for y in 0..menu_h {
        let row_start = (y * width) as usize;
        let row_end = row_start + width as usize;
        if y == menu_h - 1 {
            bb[row_start..row_end].fill(MENU_BAR_BORDER_COLOR);
        } else {
            bb[row_start..row_end].fill(MENU_BAR_COLOR);
        }
    }

    // Branding & menu items: "● AGENTIC OS   File   Edit   View   Apps   Help"
    crate::text::draw_text_to_buffer(bb, width, height, 8, 3, b"AGENTIC OS", 0x0038_BDF8, MENU_BAR_COLOR);
    crate::text::draw_text_to_buffer(bb, width, height, 110, 3, b"File   Edit   View   Apps   Help", 0x0094_A3B8, MENU_BAR_COLOR);

    // System info badges on top right
    let badge_text = b"[RAM: 256MB]  [Ring-3 Microkernel]";
    let badge_x = width.saturating_sub((badge_text.len() as u32 * 8) + 12);
    if badge_x > 380 {
        crate::text::draw_text_to_buffer(bb, width, height, badge_x, 3, badge_text, 0x0038_BDF8, MENU_BAR_COLOR);
    }

    // Bottom helper status bar
    let footer_y = height.saturating_sub(20);
    if footer_y > menu_h + 50 {
        let tip = b"Tip: Click window to focus | Drag title bar to move | Ctrl+Alt+G release mouse";
        let tip_x = (width.saturating_sub(tip.len() as u32 * 8)) / 2;
        crate::text::draw_text_to_buffer(bb, width, height, tip_x, footer_y + 2, tip, 0x0064_748B, DESKTOP_BG_COLOR);
    }

    // Flush entire backbuffer to physical GOP framebuffer using fast scanline blit!
    for y in 0..height {
        let src_ptr = bb.as_ptr().add((y * width) as usize);
        put_fb_scanline_fast(fb_phys_base, ppsl, 0, y, src_ptr, width);
    }
    core::arch::asm!("sfence", options(nomem, nostack));
    draw_cursor(fb_phys_base, ppsl, width, height);
}

pub fn register(surface_object: ObjectId, x: i32, y: i32, width: u32, height: u32, title: &[u8]) {
    let mut t = [0u8; MAX_TITLE_LEN];
    let n = title.len().min(MAX_TITLE_LEN);
    t[..n].copy_from_slice(&title[..n]);

    let mut title_buffer = vec![TITLE_BAR_COLOR; width as usize * TITLE_BAR_HEIGHT as usize];

    // Bottom border of title bar
    let border_start = ((TITLE_BAR_HEIGHT - 1) * width) as usize;
    title_buffer[border_start..border_start + width as usize].fill(TITLE_BAR_BORDER_COLOR);

    // Modern Mac/NeXT-style action dots: Red (Close), Amber (Minimize), Green (Maximize)
    if width >= 40 {
        draw_circle_to_buffer(&mut title_buffer, width, TITLE_BAR_HEIGHT, 9, 9, 3, 0x00EF_4444); // Red
        draw_circle_to_buffer(&mut title_buffer, width, TITLE_BAR_HEIGHT, 19, 9, 3, 0x00F5_9E0B); // Amber
        draw_circle_to_buffer(&mut title_buffer, width, TITLE_BAR_HEIGHT, 29, 9, 3, 0x0010_B981); // Emerald
    }

    let text_x = if width >= 40 { 38 } else { 4 };
    unsafe {
        crate::text::draw_text_to_buffer(&mut title_buffer, width, TITLE_BAR_HEIGHT, text_x, 2, &t[..n], TITLE_FG, TITLE_BAR_COLOR);
    }

    windows_mut().push(Window {
        surface_object,
        width,
        height,
        x,
        y,
        buffer: vec![0u32; (width as usize) * (height as usize)],
        title_buffer,
    });
    klog_info!("WINDOW_REGISTERED surface={} pos=({},{}) size=({},{})", surface_object, x, y, width, height);
}

fn find_mut(surface_object: ObjectId) -> Option<&'static mut Window> {
    windows_mut().iter_mut().find(|w| w.surface_object == surface_object)
}

pub fn exists(surface_object: ObjectId) -> bool {
    windows_mut().iter().any(|w| w.surface_object == surface_object)
}

pub fn raise_window(surface_object: ObjectId) -> bool {
    let windows = windows_mut();
    if let Some(pos) = windows.iter().position(|w| w.surface_object == surface_object) {
        if pos + 1 < windows.len() {
            let win = windows.remove(pos);
            windows.push(win);
            klog_info!("WINDOW_RAISED surface={}", surface_object);
            return true;
        }
    }
    false
}

pub fn unregister_window(surface_object: ObjectId, fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) -> bool {
    let windows = windows_mut();
    if let Some(pos) = windows.iter().position(|w| w.surface_object == surface_object) {
        let w = windows.remove(pos);
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let total_h = w.height + TITLE_BAR_HEIGHT;
        unsafe {
            redraw_rect(fb_phys_base, ppsl, fb_width, fb_height, w.x, bar_y, w.width, total_h);
            draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
        }
        klog_info!("WINDOW_UNREGISTERED surface={}", surface_object);
        true
    } else {
        false
    }
}

pub fn fill(surface_object: ObjectId, color: u32) -> bool {
    match find_mut(surface_object) {
        Some(w) => {
            w.buffer.fill(color);
            true
        }
        None => false,
    }
}

pub fn width_height(surface_object: ObjectId) -> Option<(u32, u32)> {
    find_mut(surface_object).map(|w| (w.width, w.height))
}

pub fn draw_text(surface_object: ObjectId, x: u32, y: u32, text: &[u8], fg: u32, bg: u32) -> bool {
    match find_mut(surface_object) {
        Some(w) => {
            unsafe { crate::text::draw_text_to_buffer(&mut w.buffer, w.width, w.height, x, y, text, fg, bg) };
            true
        }
        None => false,
    }
}

pub fn draw_bitmap(
    surface_object: ObjectId,
    x: u32,
    y: u32,
    bm_width: u32,
    bm_height: u32,
    bitmap_data: &[u8],
    fg: u32,
    bg: u32,
) -> bool {
    match find_mut(surface_object) {
        Some(w) => {
            let pitch = ((bm_width + 7) / 8) as usize;
            for row in 0..bm_height {
                let py = y + row;
                if py >= w.height {
                    break;
                }
                for col in 0..bm_width {
                    let px = x + col;
                    if px >= w.width {
                        break;
                    }
                    let byte_idx = (row as usize) * pitch + (col as usize / 8);
                    if byte_idx >= bitmap_data.len() {
                        break;
                    }
                    let bit_set = (bitmap_data[byte_idx] & (0x80 >> (col % 8))) != 0;
                    if bit_set {
                        w.buffer[(py * w.width + px) as usize] = fg;
                    } else if bg != 0 {
                        w.buffer[(py * w.width + px) as usize] = bg;
                    }
                }
            }
            true
        }
        None => false,
    }
}

pub fn move_window(surface_object: ObjectId, new_x: i32, new_y: i32, fb_width: u32, fb_height: u32) -> bool {
    match find_mut(surface_object) {
        Some(w) => {
            let min_y = TITLE_BAR_HEIGHT as i32;
            let max_x = (fb_width as i32 - w.width as i32).max(0);
            let max_y = (fb_height as i32 - w.height as i32).max(0);
            w.x = new_x.clamp(0, max_x);
            w.y = new_y.clamp(min_y, max_y);
            klog_info!("WINDOW_MOVED surface={} pos=({},{})", surface_object, w.x, w.y);
            true
        }
        None => false,
    }
}

/// Fast scanline blit: copies `pixel_count` u32 pixels in a single contiguous memory copy.
unsafe fn put_fb_scanline_fast(fb_phys_base: u64, ppsl: u32, sx: u32, sy: u32, src: *const u32, pixel_count: u32) {
    if pixel_count == 0 {
        return;
    }
    let byte_offset = (sy as u64 * ppsl as u64 + sx as u64) * 4;
    let vaddr = crate::vmm::map_framebuffer_page(fb_phys_base + byte_offset) as *mut u32;
    core::ptr::copy_nonoverlapping(src, vaddr, pixel_count as usize);
}

/// Blits a window's title bar and content buffer into the RAM BACKBUFFER
unsafe fn blit_window_to_backbuffer(w: &Window, bb: &mut [u32], fb_width: u32, fb_height: u32) {
    let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
    for py in 0..TITLE_BAR_HEIGHT as i32 {
        let sy = bar_y + py;
        if sy < 0 || sy as u32 >= fb_height {
            continue;
        }
        let x_start = if w.x < 0 { (-w.x) as u32 } else { 0 };
        let x_end = (w.width).min(fb_width.saturating_sub(w.x.max(0) as u32));
        if x_end > x_start {
            let sx = (w.x + x_start as i32) as u32;
            let count = (x_end - x_start) as usize;
            let src_off = (py as u32 * w.width + x_start) as usize;
            let dst_off = (sy as u32 * fb_width + sx) as usize;
            bb[dst_off..dst_off + count].copy_from_slice(&w.title_buffer[src_off..src_off + count]);
        }
    }

    for py in 0..w.height as i32 {
        let sy = w.y + py;
        if sy < 0 || sy as u32 >= fb_height {
            continue;
        }
        let x_start = if w.x < 0 { (-w.x) as u32 } else { 0 };
        let x_end = (w.width).min(fb_width.saturating_sub(w.x.max(0) as u32));
        if x_end > x_start {
            let sx = (w.x + x_start as i32) as u32;
            let count = (x_end - x_start) as usize;
            let src_off = (py as u32 * w.width + x_start) as usize;
            let dst_off = (sy as u32 * fb_width + sx) as usize;
            bb[dst_off..dst_off + count].copy_from_slice(&w.buffer[src_off..src_off + count]);
        }
    }
}

/// Full compositing pass: composites all windows into RAM BACKBUFFER,
/// then flushes each window's bounding scanlines to the physical GOP framebuffer.
pub unsafe fn present(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) {
    let bb = ensure_backbuffer(fb_width, fb_height);
    for w in windows_mut().iter() {
        blit_window_to_backbuffer(w, bb, fb_width, fb_height);
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let total_h = w.height + TITLE_BAR_HEIGHT;
        for py in 0..total_h as i32 {
            let sy = bar_y + py;
            if sy < 0 || sy as u32 >= fb_height {
                continue;
            }
            let x_start = if w.x < 0 { (-w.x) as u32 } else { 0 };
            let x_end = (w.width).min(fb_width.saturating_sub(w.x.max(0) as u32));
            if x_end > x_start {
                let sx = (w.x + x_start as i32) as u32;
                let count = x_end - x_start;
                let src_ptr = bb.as_ptr().add((sy as u32 * fb_width + sx) as usize);
                put_fb_scanline_fast(fb_phys_base, ppsl, sx, sy as u32, src_ptr, count);
            }
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
    draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
}

/// Partial present for typing/scrolling: blits only the modified scanlines
/// into BACKBUFFER and the physical GOP framebuffer.
pub unsafe fn present_partial(
    fb_phys_base: u64,
    ppsl: u32,
    fb_width: u32,
    fb_height: u32,
    surface_object: ObjectId,
    local_y: u32,
    local_height: u32,
) -> bool {
    let windows = windows_mut();
    let Some(w_idx) = windows.iter().position(|w| w.surface_object == surface_object) else {
        return false;
    };
    let w = &windows[w_idx];
    let end_y = (local_y + local_height).min(w.height);

    let dirty_x0 = w.x;
    let dirty_x1 = w.x + w.width as i32;
    let dirty_y0 = w.y + local_y as i32;
    let dirty_y1 = w.y + end_y as i32;

    let is_occluded = windows[w_idx + 1..].iter().any(|other| {
        let other_x0 = other.x;
        let other_x1 = other.x + other.width as i32;
        let other_y0 = other.y - TITLE_BAR_HEIGHT as i32;
        let other_y1 = other.y + other.height as i32;
        !(dirty_x1 <= other_x0 || dirty_x0 >= other_x1 || dirty_y1 <= other_y0 || dirty_y0 >= other_y1)
    });

    if is_occluded {
        present(fb_phys_base, ppsl, fb_width, fb_height);
        return true;
    }

    let bb = ensure_backbuffer(fb_width, fb_height);
    let cx = CURSOR_X.load(Ordering::SeqCst);
    let cy = CURSOR_Y.load(Ordering::SeqCst);
    let mut cursor_affected = false;

    for py in local_y..end_y {
        let sy = w.y + py as i32;
        if sy < 0 || sy as u32 >= fb_height {
            continue;
        }
        let x_start = if w.x < 0 { (-w.x) as u32 } else { 0 };
        let x_end = (w.width).min(fb_width.saturating_sub(w.x.max(0) as u32));
        if x_end <= x_start {
            continue;
        }
        let sx = (w.x + x_start as i32) as u32;
        let count = x_end - x_start;
        let src_offset = (py * w.width + x_start) as usize;
        let src_ptr = w.buffer.as_ptr().add(src_offset);

        // Update RAM backbuffer
        let bb_offset = (sy as u32 * fb_width + sx) as usize;
        bb[bb_offset..bb_offset + count as usize].copy_from_slice(&w.buffer[src_offset..src_offset + count as usize]);

        // Blit scanline to physical GOP framebuffer
        put_fb_scanline_fast(fb_phys_base, ppsl, sx, sy as u32, src_ptr, count);

        if sy >= cy && sy < cy + CURSOR_H as i32 && (sx as i32) < cx + CURSOR_W as i32 && (sx as i32 + count as i32) > cx {
            cursor_affected = true;
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
    if cursor_affected {
        draw_cursor_at(fb_phys_base, ppsl, fb_width, fb_height, cx, cy);
    }
    true
}

fn point_in_rect(px: i32, py: i32, rx: i32, ry: i32, rw: u32, rh: u32) -> bool {
    px >= rx && px < rx + rw as i32 && py >= ry && py < ry + rh as i32
}

fn topmost_titlebar_at(x: i32, y: i32) -> Option<ObjectId> {
    windows_mut().iter().rev().find_map(|w| {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        point_in_rect(x, y, w.x, bar_y, w.width, TITLE_BAR_HEIGHT).then_some(w.surface_object)
    })
}

fn topmost_content_at(x: i32, y: i32) -> Option<ObjectId> {
    windows_mut().iter().rev().find_map(|w| point_in_rect(x, y, w.x, w.y, w.width, w.height).then_some(w.surface_object))
}

// Crisp 12x18 Arrow Pointer with Black Outline and White Body
const CURSOR_W: u32 = 12;
const CURSOR_H: u32 = 18;

#[rustfmt::skip]
const CURSOR_OUTLINE: [u16; 18] = [
    0b1000_0000_0000,
    0b1100_0000_0000,
    0b1110_0000_0000,
    0b1111_0000_0000,
    0b1111_1000_0000,
    0b1111_1100_0000,
    0b1111_1110_0000,
    0b1111_1111_0000,
    0b1111_1111_1000,
    0b1111_1111_1100,
    0b1111_1111_1110,
    0b1111_1111_0000,
    0b1111_0111_1000,
    0b1110_0011_1000,
    0b1100_0001_1100,
    0b1000_0001_1100,
    0b0000_0000_1110,
    0b0000_0000_0110,
];

#[rustfmt::skip]
const CURSOR_INTERIOR: [u16; 18] = [
    0b0000_0000_0000,
    0b0100_0000_0000,
    0b0110_0000_0000,
    0b0111_0000_0000,
    0b0111_1000_0000,
    0b0111_1100_0000,
    0b0111_1110_0000,
    0b0111_1111_0000,
    0b0111_1111_0000,
    0b0111_1110_0000,
    0b0111_1100_0000,
    0b0110_1100_0000,
    0b0100_0110_0000,
    0b0000_0011_0000,
    0b0000_0001_1000,
    0b0000_0001_1000,
    0b0000_0000_0100,
    0b0000_0000_0000,
];

/// Restores the rectangle under the cursor directly from the pristine RAM backbuffer
unsafe fn restore_cursor_rect(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32, cx: i32, cy: i32) {
    let bb = ensure_backbuffer(fb_width, fb_height);
    for row in 0..CURSOR_H {
        let sy = cy + row as i32;
        if sy < 0 || sy as u32 >= fb_height {
            continue;
        }
        let x_start = cx.max(0) as u32;
        let x_end = (cx + CURSOR_W as i32).clamp(0, fb_width as i32) as u32;
        if x_end <= x_start {
            continue;
        }
        let count = x_end - x_start;
        let src_ptr = bb.as_ptr().add((sy as u32 * fb_width + x_start) as usize);
        put_fb_scanline_fast(fb_phys_base, ppsl, x_start, sy as u32, src_ptr, count);
    }
}

/// Blits the cursor sprite over the underlying pixels at (cx, cy)
unsafe fn draw_cursor_at(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32, cx: i32, cy: i32) {
    let bb = ensure_backbuffer(fb_width, fb_height);
    let mut line_buf = [0u32; CURSOR_W as usize];

    for row in 0..CURSOR_H {
        let sy = cy + row as i32;
        if sy < 0 || sy as u32 >= fb_height {
            continue;
        }
        let x_start = cx.max(0) as u32;
        let x_end = (cx + CURSOR_W as i32).clamp(0, fb_width as i32) as u32;
        if x_end <= x_start {
            continue;
        }
        let count = x_end - x_start;
        let bb_off = (sy as u32 * fb_width + x_start) as usize;
        line_buf[..count as usize].copy_from_slice(&bb[bb_off..bb_off + count as usize]);

        let outline_bits = CURSOR_OUTLINE[row as usize];
        let interior_bits = CURSOR_INTERIOR[row as usize];

        for col in 0..count {
            let actual_col = (x_start as i32 - cx) as u32 + col;
            if actual_col >= CURSOR_W {
                continue;
            }
            let shift = CURSOR_W - 1 - actual_col;
            if (interior_bits >> shift) & 1 != 0 {
                line_buf[col as usize] = 0x00FF_FFFF; // Crisp White body
            } else if (outline_bits >> shift) & 1 != 0 {
                line_buf[col as usize] = 0x0000_0000; // Black outline
            }
        }
        put_fb_scanline_fast(fb_phys_base, ppsl, x_start, sy as u32, line_buf.as_ptr(), count);
    }
    core::arch::asm!("sfence", options(nomem, nostack));
}

unsafe fn draw_cursor(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) {
    let cx = CURSOR_X.load(Ordering::SeqCst);
    let cy = CURSOR_Y.load(Ordering::SeqCst);
    draw_cursor_at(fb_phys_base, ppsl, fb_width, fb_height, cx, cy);
}

/// Redraws an arbitrary bounding box (e.g. vacated area after window drag or close)
/// using fast contiguous memory fills and scanline copies.
unsafe fn redraw_rect(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32, rx: i32, ry: i32, rw: u32, rh: u32) {
    let rx0 = rx.max(0);
    let ry0 = ry.max(0);
    let rx1 = (rx + rw as i32).min(fb_width as i32);
    let ry1 = (ry + rh as i32).min(fb_height as i32);
    if rx0 >= rx1 || ry0 >= ry1 {
        return;
    }
    let count = (rx1 - rx0) as usize;
    let bb = ensure_backbuffer(fb_width, fb_height);

    // 1. Restore background in BACKBUFFER
    for py in ry0..ry1 {
        let bg = if (py as u32) < MENU_BAR_HEIGHT {
            if py as u32 == MENU_BAR_HEIGHT - 1 {
                MENU_BAR_BORDER_COLOR
            } else {
                MENU_BAR_COLOR
            }
        } else {
            DESKTOP_BG_COLOR
        };
        let row_off = (py as u32 * fb_width + rx0 as u32) as usize;
        bb[row_off..row_off + count].fill(bg);
    }

    if ry0 < MENU_BAR_HEIGHT as i32 {
        crate::text::draw_text_to_buffer(bb, fb_width, fb_height, 8, 3, b"AGENTIC OS", 0x0038_BDF8, MENU_BAR_COLOR);
        crate::text::draw_text_to_buffer(bb, fb_width, fb_height, 110, 3, b"File   Edit   View   Apps   Help", 0x0094_A3B8, MENU_BAR_COLOR);
        let badge_text = b"[RAM: 256MB]  [Ring-3 Microkernel]";
        let badge_x = fb_width.saturating_sub((badge_text.len() as u32 * 8) + 12);
        if badge_x > 380 {
            crate::text::draw_text_to_buffer(bb, fb_width, fb_height, badge_x, 3, badge_text, 0x0038_BDF8, MENU_BAR_COLOR);
        }
    }

    let footer_y = fb_height.saturating_sub(20);
    if (ry1 as u32) > footer_y && footer_y > MENU_BAR_HEIGHT + 50 {
        let tip = b"Tip: Click window to focus | Drag title bar to move | Ctrl+Alt+G release mouse";
        let tip_x = (fb_width.saturating_sub(tip.len() as u32 * 8)) / 2;
        crate::text::draw_text_to_buffer(bb, fb_width, fb_height, tip_x, footer_y + 2, tip, 0x0064_748B, DESKTOP_BG_COLOR);
    }

    // 2. Re-blit overlapping windows in z-order
    for w in windows_mut().iter() {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let win_y0 = bar_y;
        let win_y1 = w.y + w.height as i32;
        let win_x0 = w.x;
        let win_x1 = w.x + w.width as i32;
        if rx1 <= win_x0 || rx0 >= win_x1 || ry1 <= win_y0 || ry0 >= win_y1 {
            continue;
        }
        if bar_y >= 0 {
            let by0 = ry0.max(bar_y);
            let by1 = ry1.min(bar_y + TITLE_BAR_HEIGHT as i32);
            let bx0 = rx0.max(w.x);
            let bx1 = rx1.min(w.x + w.width as i32);
            if bx1 > bx0 {
                let slice_w = (bx1 - bx0) as usize;
                for sy in by0..by1 {
                    let local_y = (sy - bar_y) as u32;
                    let local_x = (bx0 - w.x) as u32;
                    let src_off = (local_y * w.width + local_x) as usize;
                    let dst_off = (sy as u32 * fb_width + bx0 as u32) as usize;
                    bb[dst_off..dst_off + slice_w].copy_from_slice(&w.title_buffer[src_off..src_off + slice_w]);
                }
            }
        }
        let cy0 = ry0.max(w.y);
        let cy1 = ry1.min(w.y + w.height as i32);
        let cx0 = rx0.max(w.x);
        let cx1 = rx1.min(w.x + w.width as i32);
        if cx1 > cx0 {
            let slice_w = (cx1 - cx0) as usize;
            for sy in cy0..cy1 {
                let local_y = (sy - w.y) as u32;
                let local_x = (cx0 - w.x) as u32;
                let src_off = (local_y * w.width + local_x) as usize;
                let dst_off = (sy as u32 * fb_width + cx0 as u32) as usize;
                bb[dst_off..dst_off + slice_w].copy_from_slice(&w.buffer[src_off..src_off + slice_w]);
            }
        }
    }

    // 3. Flush the damaged scanlines to physical GOP framebuffer
    for py in ry0..ry1 {
        let src_ptr = bb.as_ptr().add((py as u32 * fb_width + rx0 as u32) as usize);
        put_fb_scanline_fast(fb_phys_base, ppsl, rx0 as u32, py as u32, src_ptr, (rx1 - rx0) as u32);
    }
    core::arch::asm!("sfence", options(nomem, nostack));
}

/// Real PS/2 mouse event handler with independent cursor overlay and zero window recomposition.
pub fn report_mouse(dx: i32, dy: i32, left_down: bool) {
    let Some((fb_phys_base, ppsl, fb_width, fb_height)) = crate::compositor::get_fb_params() else {
        return;
    };

    let old_x = CURSOR_X.load(Ordering::SeqCst);
    let old_y = CURSOR_Y.load(Ordering::SeqCst);
    let new_x = (old_x + dx).clamp(0, fb_width as i32 - 1);
    let new_y = (old_y + dy).clamp(0, fb_height as i32 - 1);
    CURSOR_X.store(new_x, Ordering::SeqCst);
    CURSOR_Y.store(new_y, Ordering::SeqCst);

    let was_down = LEFT_BUTTON_DOWN.swap(left_down, Ordering::SeqCst);
    let press_edge = left_down && !was_down;
    let mut window_moved = false;
    let mut window_raised = false;

    unsafe {
        if press_edge {
            if let Some(target) = topmost_titlebar_at(new_x, new_y) {
                klog_info!("WINDOW_CLICK_TITLEBAR_FOCUS surface={}", target);
                crate::input_routing::set_focus(target);
                if raise_window(target) {
                    window_raised = true;
                }
                if let Some(w) = find_mut(target) {
                    klog_info!("WINDOW_DRAG_START surface={}", target);
                    DRAGGING = Some((target, new_x - w.x, new_y - w.y));
                }
            } else if let Some(target) = topmost_content_at(new_x, new_y) {
                klog_info!("WINDOW_CLICK_FOCUS surface={}", target);
                crate::input_routing::set_focus(target);
                if raise_window(target) {
                    window_raised = true;
                }
            }
        }
        if !left_down {
            DRAGGING = None;
        }

        let mut old_win_rect: Option<(i32, i32, u32, u32)> = None;
        if let Some((dragging_object, off_x, off_y)) = DRAGGING {
            if let Some(w) = find_mut(dragging_object) {
                let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
                let total_h = w.height + TITLE_BAR_HEIGHT;
                old_win_rect = Some((w.x, bar_y, w.width, total_h));
            }
            window_moved = move_window(dragging_object, new_x - off_x, new_y - off_y, fb_width, fb_height);
        }

        if window_moved || window_raised {
            // Restore vacated window area in BACKBUFFER and flush
            if let Some((ox, oy, ow, oh)) = old_win_rect {
                redraw_rect(fb_phys_base, ppsl, fb_width, fb_height, ox, oy, ow, oh);
            }
            present(fb_phys_base, ppsl, fb_width, fb_height);
        } else {
            // Buttery-smooth mouse motion: restore old cursor 12x18 rect from RAM backbuffer,
            // then blit new cursor sprite at new coordinates. Zero window recomposition!
            restore_cursor_rect(fb_phys_base, ppsl, fb_width, fb_height, old_x, old_y);
            draw_cursor_at(fb_phys_base, ppsl, fb_width, fb_height, new_x, new_y);
        }
    }
}
