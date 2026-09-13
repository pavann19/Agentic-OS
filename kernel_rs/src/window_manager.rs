//! Real window objects: replaces the direct "a Surface capability's
//! syscall writes land straight on the real framebuffer at a fixed,
//! baked-in position" model with an actual per-window OWN backing
//! buffer (plain RAM, `alloc::vec::Vec<u32>`) plus a real title bar,
//! composited onto the real framebuffer by `present()`.
//!
//! This is what makes a window a real, independent object rather than
//! just a rectangle a syscall bounds-checks against: its content lives
//! in its own memory, so moving it (`move_window`) is a real position
//! update followed by recompositing already-drawn pixels — never a
//! request back to the client process to redraw itself.
//!
//! Real, disclosed scope: registration order is z-order (later
//! registered = drawn on top) — a fixed, deterministic policy, not
//! click-to-raise or any dynamic reordering. There is still no mouse in
//! this kernel (Phase 12's own disclosed gap), so nothing here decides
//! which window is "on top" interactively — that's real, separate
//! follow-up work once mouse input exists, not silently pretended to
//! be done.

use crate::capability::ObjectId;
use crate::klog_info;
use alloc::vec;
use alloc::vec::Vec;

const TITLE_BAR_HEIGHT: u32 = 12;
const TITLE_BAR_COLOR: u32 = 0x0040_4040;
const TITLE_FG: u32 = 0x00FF_FFFF;
const MAX_TITLE_LEN: usize = 24;

pub struct Window {
    surface_object: ObjectId,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    title: [u8; MAX_TITLE_LEN],
    title_len: usize,
    /// This window's OWN content pixels — row-major, `width * height`
    /// entries. Every `fill`/`draw_text` call below writes ONLY here;
    /// the real framebuffer is touched only by `present`.
    buffer: Vec<u32>,
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

/// Real, one-time setup: registers `surface_object` (an already-minted
/// `Surface` capability's real object id) as an actual window with its
/// own backing buffer, at an initial real screen position. Called only
/// by the same kernel-side spawn code that already mints the Surface
/// capability itself (`compositor.rs`/`terminal.rs`) — a window process
/// never registers itself, matching this kernel's existing "capability
/// grants and window/focus setup are kernel-decided, never
/// self-declared" discipline (see `input_routing.rs`'s own doc).
pub fn register(surface_object: ObjectId, x: i32, y: i32, width: u32, height: u32, title: &[u8]) {
    let mut t = [0u8; MAX_TITLE_LEN];
    let n = title.len().min(MAX_TITLE_LEN);
    t[..n].copy_from_slice(&title[..n]);
    windows_mut().push(Window {
        surface_object,
        width,
        height,
        x,
        y,
        title: t,
        title_len: n,
        buffer: vec![0u32; (width as usize) * (height as usize)],
    });
    klog_info!("WINDOW_REGISTERED surface={} pos=({},{}) size=({},{})", surface_object, x, y, width, height);
}

fn find_mut(surface_object: ObjectId) -> Option<&'static mut Window> {
    windows_mut().iter_mut().find(|w| w.surface_object == surface_object)
}

pub fn exists(surface_object: ObjectId) -> bool {
    windows_mut().iter().any(|w| w.surface_object == surface_object)
}

/// Real, bounds-checked fill of a window's OWN backing buffer (never
/// the real framebuffer) — the buffer-side counterpart of the old
/// direct `syscall_fill_surface` framebuffer write.
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

/// Real, bounds-checked text blit into a window's OWN backing buffer at
/// LOCAL (window-relative) coordinates.
pub fn draw_text(surface_object: ObjectId, x: u32, y: u32, text: &[u8], fg: u32, bg: u32) -> bool {
    match find_mut(surface_object) {
        Some(w) => {
            unsafe { crate::text::draw_text_to_buffer(&mut w.buffer, w.width, w.height, x, y, text, fg, bg) };
            true
        }
        None => false,
    }
}

/// Real window movement: updates this window's own on-screen position
/// (clamped so the title bar always stays on screen) — its content
/// buffer is untouched, so the NEXT `present()` simply blits the exact
/// same pixels at the new location.
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

unsafe fn put_fb_pixel(fb_phys_base: u64, ppsl: u32, x: u32, y: u32, color: u32) {
    let byte_offset = (y as u64 * ppsl as u64 + x as u64) * 4;
    let vaddr = crate::vmm::map_mmio_page(fb_phys_base + byte_offset);
    core::ptr::write_volatile(vaddr as *mut u32, color);
}

/// Real compositing pass: blits every registered window's own backing
/// buffer onto the real framebuffer at its CURRENT (possibly just
/// moved) position, plus a real title bar (solid fill + real PSF1
/// text) above it. This is the only place any window's content ever
/// reaches the real framebuffer.
pub unsafe fn present(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) {
    for w in windows_mut().iter() {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        if bar_y >= 0 {
            for py in 0..TITLE_BAR_HEIGHT as i32 {
                let sy = bar_y + py;
                if sy < 0 || sy as u32 >= fb_height {
                    continue;
                }
                for px in 0..w.width as i32 {
                    let sx = w.x + px;
                    if sx < 0 || sx as u32 >= fb_width {
                        continue;
                    }
                    put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, TITLE_BAR_COLOR);
                }
            }
            let title_buf_width = w.width;
            let mut title_row = vec![0u32; title_buf_width as usize * TITLE_BAR_HEIGHT as usize];
            title_row.fill(TITLE_BAR_COLOR);
            crate::text::draw_text_to_buffer(&mut title_row, title_buf_width, TITLE_BAR_HEIGHT, 2, 2, &w.title[..w.title_len], TITLE_FG, TITLE_BAR_COLOR);
            for py in 0..TITLE_BAR_HEIGHT as i32 {
                let sy = bar_y + py;
                if sy < 0 || sy as u32 >= fb_height {
                    continue;
                }
                for px in 0..w.width as i32 {
                    let sx = w.x + px;
                    if sx < 0 || sx as u32 >= fb_width {
                        continue;
                    }
                    let color = title_row[(py as u32 * title_buf_width + px as u32) as usize];
                    put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, color);
                }
            }
        }
        for py in 0..w.height as i32 {
            let sy = w.y + py;
            if sy < 0 || sy as u32 >= fb_height {
                continue;
            }
            for px in 0..w.width as i32 {
                let sx = w.x + px;
                if sx < 0 || sx as u32 >= fb_width {
                    continue;
                }
                let color = w.buffer[(py as u32 * w.width + px as u32) as usize];
                put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, color);
            }
        }
    }
}
