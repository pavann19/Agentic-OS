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

// Phase 5.1 & 5.2: Decoupled presentation & real damage region tracking
static CURSOR_DIRTY: AtomicBool = AtomicBool::new(false);
static WINDOW_DIRTY: AtomicBool = AtomicBool::new(false);

pub type DamageRect = kernel_common::geometry::Rect;

static mut GLOBAL_DAMAGE: crate::damage::DamageRegion = crate::damage::DamageRegion::new();

pub fn add_damage_rect(rect: DamageRect) {
    unsafe {
        #[allow(static_mut_refs)]
        let slot = &mut *&raw mut GLOBAL_DAMAGE;
        slot.add_rect(rect);
        mark_window_dirty();
    }
}

pub fn clear_damage_rects() {
    unsafe {
        #[allow(static_mut_refs)]
        let slot = &mut *&raw mut GLOBAL_DAMAGE;
        slot.clear();
    }
}

#[inline(always)]
pub fn mark_cursor_dirty() {
    CURSOR_DIRTY.store(true, Ordering::Release);
}

#[inline(always)]
pub fn mark_window_dirty() {
    WINDOW_DIRTY.store(true, Ordering::Release);
}

#[inline(always)]
pub fn is_window_dirty() -> bool {
    WINDOW_DIRTY.load(Ordering::Acquire)
}

#[inline(always)]
pub fn is_cursor_dirty() -> bool {
    CURSOR_DIRTY.load(Ordering::Acquire)
}

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

/// Base user-space virtual address assigned for mapped window shared surfaces.
pub const USER_SURFACE_BASE: u64 = 0x0000_7000_0000_0000;

/// Maps a window's backbuffer directly into a ring-3 process's address space (Phase 5.3).
/// Allows zero-copy rendering directly into the surface memory.
pub unsafe fn map_window_buffer_to_user(surface_object: ObjectId, user_pml4: u64) -> Option<(u64, u32, u32)> {
    let w = find_mut(surface_object)?;
    let user_base = USER_SURFACE_BASE + (surface_object as u64) * 0x0010_0000;
    let size_bytes = (w.width as usize) * (w.height as usize) * 4;
    let page_count = (size_bytes + 4095) / 4096;

    let buf_ptr = w.buffer.as_ptr() as u64;
    for p in 0..page_count {
        let kern_vaddr = buf_ptr + (p as u64) * 4096;
        let phys = crate::vmm::virt_to_phys(crate::vmm::kernel_pml4_phys(), kern_vaddr)?;
        let user_vaddr = user_base + (p as u64) * 4096;
        crate::vmm::map_page_in(
            user_pml4,
            user_vaddr,
            phys,
            crate::vmm::PAGE_USER | crate::vmm::PAGE_WRITABLE | crate::vmm::PAGE_NO_EXECUTE,
        );
    }
    klog_info!("WINDOW_SURFACE_MAPPED surface={} user_vaddr=0x{:x} size={} pages={}", surface_object, user_base, size_bytes, page_count);
    Some((user_base, w.width, w.height))
}

/// Commits a damaged rectangular region of a window surface (Phase 5.3).
pub fn commit_window_damage(surface_object: ObjectId, x: u32, y: u32, width: u32, height: u32) -> bool {
    let Some(w) = find_mut(surface_object) else {
        return false;
    };
    let dirty_x = w.x + x as i32;
    let dirty_y = w.y + y as i32;
    let dirty_w = width.min(w.width.saturating_sub(x));
    let dirty_h = height.min(w.height.saturating_sub(y));
    if dirty_w > 0 && dirty_h > 0 {
        add_damage_rect(DamageRect {
            x: dirty_x,
            y: dirty_y,
            width: dirty_w,
            height: dirty_h,
        });
        true
    } else {
        false
    }
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
            let clamped_x = new_x.clamp(0, max_x);
            let clamped_y = new_y.clamp(min_y, max_y);
            if w.x == clamped_x && w.y == clamped_y {
                return false;
            }
            w.x = clamped_x;
            w.y = clamped_y;
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

/// Blits a window's visible title bar and content into RAM BACKBUFFER after subtracting higher-z occluders (Phase 5.4).
unsafe fn blit_window_clipped(
    w: &Window,
    occluders: &[DamageRect],
    bb: &mut [u32],
    fb_width: u32,
    fb_height: u32,
) {
    let screen_rect = DamageRect::new(0, 0, fb_width, fb_height);

    // 1. Title bar visible pieces
    let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
    let title_rect = DamageRect::new(w.x, bar_y, w.width, TITLE_BAR_HEIGHT);
    let mut vis_title = [DamageRect::default(); 32];
    let n_title = DamageRect::compute_visible_rects(&title_rect, occluders, &mut vis_title);

    for i in 0..n_title {
        if let Some(rc) = vis_title[i].intersect(&screen_rect) {
            let slice_w = rc.width as usize;
            for sy in rc.y..rc.bottom() {
                let local_y = (sy - bar_y) as u32;
                let local_x = (rc.x - w.x) as u32;
                let src_off = (local_y * w.width + local_x) as usize;
                let dst_off = (sy as u32 * fb_width + rc.x as u32) as usize;
                bb[dst_off..dst_off + slice_w].copy_from_slice(&w.title_buffer[src_off..src_off + slice_w]);
            }
        }
    }

    // 2. Content buffer visible pieces
    let content_rect = DamageRect::new(w.x, w.y, w.width, w.height);
    let mut vis_content = [DamageRect::default(); 32];
    let n_content = DamageRect::compute_visible_rects(&content_rect, occluders, &mut vis_content);

    for i in 0..n_content {
        if let Some(rc) = vis_content[i].intersect(&screen_rect) {
            let slice_w = rc.width as usize;
            for sy in rc.y..rc.bottom() {
                let local_y = (sy - w.y) as u32;
                let local_x = (rc.x - w.x) as u32;
                let src_off = (local_y * w.width + local_x) as usize;
                let dst_off = (sy as u32 * fb_width + rc.x as u32) as usize;
                bb[dst_off..dst_off + slice_w].copy_from_slice(&w.buffer[src_off..src_off + slice_w]);
            }
        }
    }
}

/// Full compositing pass with Z-order occlusion culling and sub-rectangle clipping (Phase 5.4):
/// Composites only visible portions of all windows into RAM BACKBUFFER,
/// then flushes only the un-occluded visible rectangles to the physical GOP framebuffer.
pub unsafe fn present(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) {
    let t_start = crate::compositor_metrics::read_tsc();
    let bb = ensure_backbuffer(fb_width, fb_height);
    let screen_rect = DamageRect::new(0, 0, fb_width, fb_height);

    let t_compose_start = crate::compositor_metrics::read_tsc();
    let windows = windows_mut();
    let num_windows = windows.len();

    // Collect bounding boxes of all windows for occlusion computation
    let mut win_bounds = [DamageRect::default(); 16];
    for (i, w) in windows.iter().enumerate().take(16) {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let total_h = w.height + TITLE_BAR_HEIGHT;
        win_bounds[i] = DamageRect::new(w.x, bar_y, w.width, total_h);
    }

    // Compose each window in z-order, clipped against all higher-z windows
    for i in 0..num_windows {
        let occluders = &win_bounds[i + 1..num_windows.min(16)];
        blit_window_clipped(&windows[i], occluders, bb, fb_width, fb_height);
    }
    let t_compose_end = crate::compositor_metrics::read_tsc();

    let mut damaged_rects = 0usize;
    let mut damaged_scanlines = 0usize;
    let mut damaged_pixels = 0usize;

    let t_flush_start = crate::compositor_metrics::read_tsc();
    for i in 0..num_windows {
        let w = &windows[i];
        let occluders = &win_bounds[i + 1..num_windows.min(16)];

        // Compute visible rects for title + content
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let title_rect = DamageRect::new(w.x, bar_y, w.width, TITLE_BAR_HEIGHT);
        let content_rect = DamageRect::new(w.x, w.y, w.width, w.height);

        let mut vis = [DamageRect::default(); 32];
        let n_title = DamageRect::compute_visible_rects(&title_rect, occluders, &mut vis);
        for k in 0..n_title {
            let r = vis[k];
            if let Some(rc) = r.intersect(&screen_rect) {
                damaged_rects += 1;
                for sy in rc.y..rc.bottom() {
                    let src_ptr = bb.as_ptr().add((sy as u32 * fb_width + rc.x as u32) as usize);
                    put_fb_scanline_fast(fb_phys_base, ppsl, rc.x as u32, sy as u32, src_ptr, rc.width);
                    damaged_scanlines += 1;
                    damaged_pixels += rc.width as usize;
                }
            }
        }

        let n_content = DamageRect::compute_visible_rects(&content_rect, occluders, &mut vis);
        for k in 0..n_content {
            let r = vis[k];
            if let Some(rc) = r.intersect(&screen_rect) {
                damaged_rects += 1;
                for sy in rc.y..rc.bottom() {
                    let src_ptr = bb.as_ptr().add((sy as u32 * fb_width + rc.x as u32) as usize);
                    put_fb_scanline_fast(fb_phys_base, ppsl, rc.x as u32, sy as u32, src_ptr, rc.width);
                    damaged_scanlines += 1;
                    damaged_pixels += rc.width as usize;
                }
            }
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
    let t_flush_end = crate::compositor_metrics::read_tsc();

    let t_cursor_start = crate::compositor_metrics::read_tsc();
    draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
    let t_cursor_end = crate::compositor_metrics::read_tsc();

    let t_end = crate::compositor_metrics::read_tsc();
    let metrics = crate::compositor_metrics::FrameMetrics {
        frame_time_cycles: t_end.saturating_sub(t_start),
        input_time_cycles: 0,
        damage_time_cycles: 0,
        compose_time_cycles: t_compose_end.saturating_sub(t_compose_start),
        flush_time_cycles: t_flush_end.saturating_sub(t_flush_start),
        cursor_time_cycles: t_cursor_end.saturating_sub(t_cursor_start),
        damaged_rects,
        damaged_scanlines,
        damaged_pixels,
    };
    crate::compositor_metrics::record_frame(&metrics);
}

/// Partial present for typing/scrolling with occlusion culling (Phase 5.4):
/// Blits only the visible, un-occluded slices into BACKBUFFER and GOP framebuffer.
/// Never falls back to full-screen redraw when partially or fully occluded!
pub unsafe fn present_partial(
    fb_phys_base: u64,
    ppsl: u32,
    fb_width: u32,
    fb_height: u32,
    surface_object: ObjectId,
    local_y: u32,
    local_height: u32,
) -> bool {
    let t_start = crate::compositor_metrics::read_tsc();
    let windows = windows_mut();
    let Some(w_idx) = windows.iter().position(|w| w.surface_object == surface_object) else {
        return false;
    };
    let w = &windows[w_idx];
    let end_y = (local_y + local_height).min(w.height);
    if local_y >= end_y {
        return false;
    }
    let actual_h = end_y - local_y;

    // Collect occluders from higher-z windows
    let mut occluders = [DamageRect::default(); 16];
    let mut occ_count = 0;
    for higher in &windows[w_idx + 1..] {
        if occ_count < 16 {
            let bar_y = higher.y - TITLE_BAR_HEIGHT as i32;
            let total_h = higher.height + TITLE_BAR_HEIGHT;
            occluders[occ_count] = DamageRect::new(higher.x, bar_y, higher.width, total_h);
            occ_count += 1;
        }
    }
    let occ_slice = &occluders[..occ_count];

    // Compute visible sub-rectangles for the damaged local rows
    let damage_rect = DamageRect::new(w.x, w.y + local_y as i32, w.width, actual_h);
    let screen_rect = DamageRect::new(0, 0, fb_width, fb_height);

    let t_damage_start = crate::compositor_metrics::read_tsc();
    let mut vis = [DamageRect::default(); 32];
    let n_vis = DamageRect::compute_visible_rects(&damage_rect, occ_slice, &mut vis);
    let t_damage_end = crate::compositor_metrics::read_tsc();

    if n_vis == 0 {
        // Completely occluded! Content is already preserved in w.buffer. Zero MMIO writes needed!
        return true;
    }

    let bb = ensure_backbuffer(fb_width, fb_height);
    let cx = CURSOR_X.load(Ordering::SeqCst);
    let cy = CURSOR_Y.load(Ordering::SeqCst);
    let mut cursor_affected = false;
    let mut damaged_scanlines = 0usize;
    let mut damaged_pixels = 0usize;

    let t_flush_start = crate::compositor_metrics::read_tsc();
    for i in 0..n_vis {
        let r = vis[i];
        if let Some(rc) = r.intersect(&screen_rect) {
            let count = rc.width;
            for sy in rc.y..rc.bottom() {
                let py = (sy - w.y) as u32;
                let local_x = (rc.x - w.x) as u32;
                let src_offset = (py * w.width + local_x) as usize;
                let dst_offset = (sy as u32 * fb_width + rc.x as u32) as usize;

                // Update RAM backbuffer
                bb[dst_offset..dst_offset + count as usize].copy_from_slice(&w.buffer[src_offset..src_offset + count as usize]);

                // Flush scanline to physical GOP framebuffer
                let src_ptr = bb.as_ptr().add(dst_offset);
                put_fb_scanline_fast(fb_phys_base, ppsl, rc.x as u32, sy as u32, src_ptr, count);
                damaged_scanlines += 1;
                damaged_pixels += count as usize;

                if sy >= cy && sy < cy + CURSOR_H as i32 && (rc.x) < cx + CURSOR_W as i32 && (rc.x + count as i32) > cx {
                    cursor_affected = true;
                }
            }
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
    let t_flush_end = crate::compositor_metrics::read_tsc();

    let t_cursor_start = crate::compositor_metrics::read_tsc();
    if cursor_affected {
        draw_cursor_at(fb_phys_base, ppsl, fb_width, fb_height, cx, cy);
    }
    let t_cursor_end = crate::compositor_metrics::read_tsc();

    let t_end = crate::compositor_metrics::read_tsc();
    let metrics = crate::compositor_metrics::FrameMetrics {
        frame_time_cycles: t_end.saturating_sub(t_start),
        input_time_cycles: 0,
        damage_time_cycles: t_damage_end.saturating_sub(t_damage_start),
        compose_time_cycles: 0,
        flush_time_cycles: t_flush_end.saturating_sub(t_flush_start),
        cursor_time_cycles: t_cursor_end.saturating_sub(t_cursor_start),
        damaged_rects: n_vis,
        damaged_scanlines,
        damaged_pixels,
    };
    crate::compositor_metrics::record_frame(&metrics);
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

    // 2. Re-blit overlapping windows in z-order with occlusion clipping (Phase 5.4/5.5)
    let target_rect = DamageRect::new(rx0, ry0, (rx1 - rx0) as u32, (ry1 - ry0) as u32);
    let windows = windows_mut();
    let num_windows = windows.len();

    let mut win_bounds = [DamageRect::default(); 16];
    for (i, w) in windows.iter().enumerate().take(16) {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let total_h = w.height + TITLE_BAR_HEIGHT;
        win_bounds[i] = DamageRect::new(w.x, bar_y, w.width, total_h);
    }

    for i in 0..num_windows {
        let w = &windows[i];
        let occluders = &win_bounds[i + 1..num_windows.min(16)];

        // Title bar inside target_rect
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        let title_rect = DamageRect::new(w.x, bar_y, w.width, TITLE_BAR_HEIGHT);
        if let Some(t_target) = title_rect.intersect(&target_rect) {
            let mut vis = [DamageRect::default(); 32];
            let n = DamageRect::compute_visible_rects(&t_target, occluders, &mut vis);
            for k in 0..n {
                let rc = vis[k];
                let slice_w = rc.width as usize;
                for sy in rc.y..rc.bottom() {
                    let local_y = (sy - bar_y) as u32;
                    let local_x = (rc.x - w.x) as u32;
                    let src_off = (local_y * w.width + local_x) as usize;
                    let dst_off = (sy as u32 * fb_width + rc.x as u32) as usize;
                    bb[dst_off..dst_off + slice_w].copy_from_slice(&w.title_buffer[src_off..src_off + slice_w]);
                }
            }
        }

        // Content buffer inside target_rect
        let content_rect = DamageRect::new(w.x, w.y, w.width, w.height);
        if let Some(c_target) = content_rect.intersect(&target_rect) {
            let mut vis = [DamageRect::default(); 32];
            let n = DamageRect::compute_visible_rects(&c_target, occluders, &mut vis);
            for k in 0..n {
                let rc = vis[k];
                let slice_w = rc.width as usize;
                for sy in rc.y..rc.bottom() {
                    let local_y = (sy - w.y) as u32;
                    let local_x = (rc.x - w.x) as u32;
                    let src_off = (local_y * w.width + local_x) as usize;
                    let dst_off = (sy as u32 * fb_width + rc.x as u32) as usize;
                    bb[dst_off..dst_off + slice_w].copy_from_slice(&w.buffer[src_off..src_off + slice_w]);
                }
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
    let t_start = crate::compositor_metrics::read_tsc();
    crate::compositor_metrics::record_mouse_event();

    // Phase 5.1: Enqueue hardware input event into lock-free ring buffer
    crate::input_queue::enqueue(crate::input_queue::InputEventKind::MouseMotion { dx, dy, left_down });

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
            // Decoupled window composition with real damage region tracking (Phase 5.2):
            // Add vacated rectangle + new window rectangle to damage accumulator.
            // Overlapping regions are automatically merged into a minimal bounding box!
            if let Some((ox, oy, ow, oh)) = old_win_rect {
                add_damage_rect(DamageRect { x: ox, y: oy, width: ow, height: oh });
            }
            if let Some((dragging_object, _, _)) = DRAGGING {
                if let Some(w) = find_mut(dragging_object) {
                    let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
                    let total_h = w.height + TITLE_BAR_HEIGHT;
                    add_damage_rect(DamageRect { x: w.x, y: bar_y, width: w.width, height: total_h });
                }
            } else {
                for w in windows_mut().iter() {
                    let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
                    let total_h = w.height + TITLE_BAR_HEIGHT;
                    add_damage_rect(DamageRect { x: w.x, y: bar_y, width: w.width, height: total_h });
                }
            }
            mark_window_dirty();
            flush_dirty_surfaces(fb_phys_base, ppsl, fb_width, fb_height);
        } else {
            // Buttery-smooth mouse motion: restore old cursor 12x18 rect from RAM backbuffer,
            // then blit new cursor sprite at new coordinates. Zero window recomposition!
            let t_cursor_start = crate::compositor_metrics::read_tsc();
            restore_cursor_rect(fb_phys_base, ppsl, fb_width, fb_height, old_x, old_y);
            draw_cursor_at(fb_phys_base, ppsl, fb_width, fb_height, new_x, new_y);
            let t_cursor_end = crate::compositor_metrics::read_tsc();

            let t_end = crate::compositor_metrics::read_tsc();
            let metrics = crate::compositor_metrics::FrameMetrics {
                frame_time_cycles: t_end.saturating_sub(t_start),
                input_time_cycles: t_cursor_start.saturating_sub(t_start),
                damage_time_cycles: 0,
                compose_time_cycles: 0,
                flush_time_cycles: 0,
                cursor_time_cycles: t_cursor_end.saturating_sub(t_cursor_start),
                damaged_rects: 1,
                damaged_scanlines: (CURSOR_H * 2) as usize,
                damaged_pixels: (CURSOR_W * CURSOR_H * 2) as usize,
            };
            crate::compositor_metrics::record_frame(&metrics);
        }
    }
}

/// Flushes pending presentations using real damage region tracking (Phase 5.2).
pub unsafe fn flush_dirty_surfaces(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) {
    if WINDOW_DIRTY.swap(false, Ordering::AcqRel) {
        #[allow(static_mut_refs)]
        let damage = &mut *&raw mut GLOBAL_DAMAGE;
        if !damage.is_empty() {
            let t_start = crate::compositor_metrics::read_tsc();
            let rect_count = damage.count();
            let mut total_scanlines = 0usize;
            let mut total_pixels = 0usize;

            for i in 0..rect_count {
                let r = damage.rects()[i];
                redraw_rect(fb_phys_base, ppsl, fb_width, fb_height, r.x, r.y, r.width, r.height);
                total_scanlines += r.height as usize;
                total_pixels += (r.width as usize) * (r.height as usize);
            }
            damage.clear();
            draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);

            let t_end = crate::compositor_metrics::read_tsc();
            let metrics = crate::compositor_metrics::FrameMetrics {
                frame_time_cycles: t_end.saturating_sub(t_start),
                input_time_cycles: 0,
                damage_time_cycles: 0,
                compose_time_cycles: 0,
                flush_time_cycles: t_end.saturating_sub(t_start),
                cursor_time_cycles: 0,
                damaged_rects: rect_count,
                damaged_scanlines: total_scanlines,
                damaged_pixels: total_pixels,
            };
            crate::compositor_metrics::record_frame(&metrics);
        } else {
            present(fb_phys_base, ppsl, fb_width, fb_height);
        }
    } else if CURSOR_DIRTY.swap(false, Ordering::AcqRel) {
        draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
    }
}

pub fn dump_metrics(scenario: &str) {
    crate::compositor_metrics::dump_summary(scenario);
}

pub fn reset_metrics() {
    crate::compositor_metrics::reset_metrics();
}
