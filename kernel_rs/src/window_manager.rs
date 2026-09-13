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
//! dynamic click-to-raise reordering (a real click still changes
//! FOCUS and can DRAG a window, see `report_mouse` below, just not
//! which window paints on top of another overlapping one).
//!
//! Real GUI mouse support (`report_mouse`): a real PS/2 mouse
//! (`user_rs/mouse_driver`, IRQ12) reports real relative deltas and
//! button state here via `SYS_MOUSE_REPORT` (syscall 22). This is the
//! kernel's own real window-manager policy, not something any app
//! decides: a press inside a window's real title-bar rect starts a
//! real drag (subsequent moves call `move_window`); a press inside a
//! window's real content rect changes real keyboard focus
//! (`input_routing::set_focus`) -- the same real, disclosed
//! click-to-focus Phase 12's own doc named as follow-up work, now
//! done.

use crate::capability::ObjectId;
use crate::klog_info;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};

const TITLE_BAR_HEIGHT: u32 = 12;
const TITLE_BAR_COLOR: u32 = 0x0040_4040;
const TITLE_FG: u32 = 0x00FF_FFFF;
const MAX_TITLE_LEN: usize = 24;

// Real desktop chrome colors -- the single, real source of truth
// `main.rs::draw_desktop_chrome` also reads, so the one-time full
// paint at boot and this module's own per-pixel dirty-rect repaint
// (`redraw_rect`, below) can never drift apart.
pub const DESKTOP_BG_COLOR: u32 = 0x00C0_C0C0;
pub const MENU_BAR_COLOR: u32 = 0x00FF_FFFF;
pub const MENU_BAR_HEIGHT: u32 = 20;

// Real cursor state -- a real, visible, moving GUI cursor, not just an
// input-routing abstraction. Plain statics (unsafe, cooperative single-
// core discipline this whole file already uses) rather than a lock:
// `report_mouse` is the only writer, always called from the mouse
// driver's own syscall dispatch, never concurrently with itself.
static CURSOR_X: AtomicI32 = AtomicI32::new(160);
static CURSOR_Y: AtomicI32 = AtomicI32::new(100);
static LEFT_BUTTON_DOWN: AtomicBool = AtomicBool::new(false);
/// `Some((surface_object, grab_offset_x, grab_offset_y))` while a real
/// title-bar drag is in progress -- the offset is the real, fixed
/// distance from the window's own top-left corner to wherever inside
/// the title bar the button went down, so the window doesn't jump to
/// have its corner snap under the cursor the instant a drag starts.
static mut DRAGGING: Option<(ObjectId, i32, i32)> = None;

pub struct Window {
    surface_object: ObjectId,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    /// This window's OWN content pixels — row-major, `width * height`
    /// entries. Every `fill`/`draw_text` call below writes ONLY here;
    /// the real framebuffer is touched only by `present`.
    buffer: Vec<u32>,
    /// Real, precomputed title-bar pixels (`width * TITLE_BAR_HEIGHT`),
    /// built ONCE at registration -- real, disclosed fix for two real
    /// problems the old "rebuild it from scratch inside every single
    /// `present()` call" approach had: (1) real, wasted per-frame cost
    /// (re-rendering the same never-changing title text on every
    /// recompositable event, including every mouse-move tick), and (2)
    /// it's what makes `redraw_rect`'s own cursor-damage-rect repaint
    /// possible at all -- that path repaints an arbitrary SUB-region,
    /// which needs a real, already-rendered title image to sample from
    /// rather than re-rendering a whole row just to read a few pixels.
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
    let mut title_buffer = vec![TITLE_BAR_COLOR; width as usize * TITLE_BAR_HEIGHT as usize];
    unsafe { crate::text::draw_text_to_buffer(&mut title_buffer, width, TITLE_BAR_HEIGHT, 2, 2, &t[..n], TITLE_FG, TITLE_BAR_COLOR) };
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

/// Real, bounds-checked 1-bit monochrome bitmap/icon blit into a window's
/// OWN backing buffer at LOCAL (window-relative) coordinates.
/// 1-bit = fg color. 0-bit = bg color (or skipped/transparent if bg == 0).
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
    let vaddr = crate::vmm::map_framebuffer_page(fb_phys_base + byte_offset);
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
                    let color = w.title_buffer[(py as u32 * w.width + px as u32) as usize];
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
    // Real, necessary correctness fix that comes WITH write-combining
    // (`vmm::map_framebuffer_page`'s own doc): WC stores can be
    // buffered by the CPU and are not guaranteed visible to any other
    // observer (the real display device included) until explicitly
    // flushed. A real `sfence` after this whole compositing pass is
    // what actually makes it visible on screen -- omitting it here
    // would make WC's speed win silently reintroduce stale/incomplete
    // frames, a real correctness regression, not a nitpick.
    core::arch::asm!("sfence", options(nomem, nostack));
    // Real GUI mouse support: the cursor is composited last, on top of
    // every window, every time anything recomposites -- otherwise a
    // keystroke's own redraw could paint window content right over
    // wherever the cursor currently sits.
    draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
}

/// Real, disclosed follow-up fix: real `rdtsc` measurement showed
/// switching the framebuffer mapping to Write-Combining made NO
/// measurable difference (~19-22M cycles either way) -- proof the real
/// bottleneck isn't the guest-side cache/PAT attribute at all, but
/// QEMU's own MMIO emulation: this framebuffer is a trapped device
/// region (a real emulated VGA/bochs display BAR, not plain RAM), so
/// EVERY individual store into it is intercepted by the emulator's own
/// device-model callback regardless of what caching policy the guest
/// declares -- no page-table attribute can make an individually
/// trapped access cheap. The only real lever left is writing FEWER
/// pixels: `present` (above) always recomposited the WHOLE window
/// (title bar + all content) even when a single keystroke only changed
/// ONE row of text. This partial-present variant blits ONLY the given
/// local row range of ONE window's content -- no title bar, no other
/// rows, no other windows -- cutting typical per-keystroke pixel
/// writes from ~68,000 (full 320x200 window + bar) down to ~5,120 (one
/// 320x16 text row). Real, disclosed scope: correct only because
/// windows in this kernel don't currently overlap (no window manager
/// z-order compositing of overlapping regions yet) -- a caller must
/// only use this when it KNOWS no other window's content or this
/// window's own title bar could have changed.
pub unsafe fn present_partial(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32, surface_object: ObjectId, local_y: u32, local_height: u32) -> bool {
    let Some(w) = find_mut(surface_object) else { return false };
    let end_y = (local_y + local_height).min(w.height);
    for py in local_y..end_y {
        let sy = w.y + py as i32;
        if sy < 0 || sy as u32 >= fb_height {
            continue;
        }
        for px in 0..w.width as i32 {
            let sx = w.x + px;
            if sx < 0 || sx as u32 >= fb_width {
                continue;
            }
            let color = w.buffer[(py * w.width + px as u32) as usize];
            put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, color);
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
    draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
    true
}

fn point_in_rect(px: i32, py: i32, rx: i32, ry: i32, rw: u32, rh: u32) -> bool {
    px >= rx && px < rx + rw as i32 && py >= ry && py < ry + rh as i32
}

/// Finds the TOPMOST window (highest z-order, i.e. last registered)
/// whose real title-bar rect contains `(x, y)` -- checked before
/// content, since a title bar can sit just above a window's own
/// content rect and the two must never both match the same point.
fn topmost_titlebar_at(x: i32, y: i32) -> Option<ObjectId> {
    windows_mut().iter().rev().find_map(|w| {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        point_in_rect(x, y, w.x, bar_y, w.width, TITLE_BAR_HEIGHT).then_some(w.surface_object)
    })
}

/// Finds the TOPMOST window whose real content rect contains `(x, y)`.
fn topmost_content_at(x: i32, y: i32) -> Option<ObjectId> {
    windows_mut().iter().rev().find_map(|w| point_in_rect(x, y, w.x, w.y, w.width, w.height).then_some(w.surface_object))
}

/// Real, small (11x16), classic-arrow cursor bitmap -- 1 = a real black
/// pixel, 0 = see-through (background/window content shows through).
/// Same real, disclosed "simple but genuine" spirit as this project's
/// other minimal-but-real UI primitives (the PSF1 glyph blit, the
/// title bar fill): a real recognizable pointer shape, not a single
/// crosshair pixel.
const CURSOR_W: usize = 11;
const CURSOR_H: usize = 16;
#[rustfmt::skip]
const CURSOR_BITMAP: [u16; CURSOR_H] = [
    0b1000_0000_000,
    0b1100_0000_000,
    0b1110_0000_000,
    0b1111_0000_000,
    0b1111_1000_000,
    0b1111_1100_000,
    0b1111_1110_000,
    0b1111_1111_000,
    0b1111_1111_100,
    0b1111_1111_110,
    0b1111_1100_000,
    0b1101_1110_000,
    0b1000_1110_000,
    0b0000_0111_000,
    0b0000_0111_000,
    0b0000_0011_000,
];

unsafe fn draw_cursor(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32) {
    let cx = CURSOR_X.load(Ordering::SeqCst);
    let cy = CURSOR_Y.load(Ordering::SeqCst);
    for row in 0..CURSOR_H {
        let bits = CURSOR_BITMAP[row];
        for col in 0..CURSOR_W {
            if (bits >> (CURSOR_W - 1 - col)) & 1 == 0 {
                continue;
            }
            let sx = cx + col as i32;
            let sy = cy + row as i32;
            if sx < 0 || sy < 0 || sx as u32 >= fb_width || sy as u32 >= fb_height {
                continue;
            }
            put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, 0x0000_0000);
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
}

/// Real PS/2 mouse event handler -- see this module's own doc for the
/// real click-to-focus/drag policy. Called once per real, decoded
/// 3-byte mouse packet (`mouse_driver`'s own syscall 22). Real,
/// disclosed cost: unlike the keyboard's own dirty-rect `present_
/// partial` path, this always recomposites the WHOLE desktop (every
/// window, full content) before drawing the cursor on top -- a real,
/// stated simplification (mouse movement isn't yet dirty-rect
/// optimized the way text redraw is), not a hidden shortcut.
/// Real, disclosed fix for TWO real problems the old "just call
/// `present()` on every mouse event" approach had: (1) real, visible
/// LAG -- a real PS/2 mouse sends dozens of packets per second while
/// moving, and `present()` recomposites the ENTIRE desktop (every
/// window's full content) on every single one; (2) real CURSOR TRAILS
/// -- `present()` only ever repaints WINDOW rectangles, never the bare
/// desktop background, so an old cursor position sitting over open
/// desktop was never actually erased, leaving a permanent smear.
///
/// Repaints ONLY the real, small rectangle the cursor's bitmap
/// actually occupies at its OLD and NEW position (background/menu-bar
/// color first, then any window content that overlaps it, in real
/// z-order) -- real background erasure AND a real, small, bounded
/// redraw instead of the whole screen. A window POSITION change
/// (dragging) is the one real exception that still needs a full
/// `present()`: the window itself moved, potentially far from this
/// event's own small cursor-damage rect, so anything less would leave
/// a real ghost of it at its old spot.
unsafe fn redraw_rect(fb_phys_base: u64, ppsl: u32, fb_width: u32, fb_height: u32, rx: i32, ry: i32, rw: u32, rh: u32) {
    let rx0 = rx.max(0);
    let ry0 = ry.max(0);
    let rx1 = (rx + rw as i32).min(fb_width as i32);
    let ry1 = (ry + rh as i32).min(fb_height as i32);
    if rx0 >= rx1 || ry0 >= ry1 {
        return;
    }
    // Real desktop background + menu bar, per pixel -- the actual fix
    // for cursor trails over bare desktop (see this function's own doc).
    for py in ry0..ry1 {
        let bg = if (py as u32) < MENU_BAR_HEIGHT { MENU_BAR_COLOR } else { DESKTOP_BG_COLOR };
        for px in rx0..rx1 {
            put_fb_pixel(fb_phys_base, ppsl, px as u32, py as u32, bg);
        }
    }
    // Real window content (title bar, then body) for every window
    // whose real rect overlaps this damage rect, in real z-order.
    for w in windows_mut().iter() {
        let bar_y = w.y - TITLE_BAR_HEIGHT as i32;
        if bar_y >= 0 {
            let by0 = ry0.max(bar_y);
            let by1 = ry1.min(bar_y + TITLE_BAR_HEIGHT as i32);
            let bx0 = rx0.max(w.x);
            let bx1 = rx1.min(w.x + w.width as i32);
            for sy in by0..by1 {
                let local_y = (sy - bar_y) as u32;
                for sx in bx0..bx1 {
                    let local_x = (sx - w.x) as u32;
                    let color = w.title_buffer[(local_y * w.width + local_x) as usize];
                    put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, color);
                }
            }
        }
        let cy0 = ry0.max(w.y);
        let cy1 = ry1.min(w.y + w.height as i32);
        let cx0 = rx0.max(w.x);
        let cx1 = rx1.min(w.x + w.width as i32);
        for sy in cy0..cy1 {
            let local_y = (sy - w.y) as u32;
            for sx in cx0..cx1 {
                let local_x = (sx - w.x) as u32;
                let color = w.buffer[(local_y * w.width + local_x) as usize];
                put_fb_pixel(fb_phys_base, ppsl, sx as u32, sy as u32, color);
            }
        }
    }
    core::arch::asm!("sfence", options(nomem, nostack));
}

pub fn report_mouse(dx: i32, dy: i32, left_down: bool) {
    let Some((fb_phys_base, ppsl, fb_width, fb_height)) = crate::compositor::get_fb_params() else {
        return; // no real framebuffer set up yet -- nothing to composite against
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

    unsafe {
        if press_edge {
            if let Some(target) = topmost_titlebar_at(new_x, new_y) {
                if let Some(w) = find_mut(target) {
                    klog_info!("WINDOW_DRAG_START surface={}", target);
                    DRAGGING = Some((target, new_x - w.x, new_y - w.y));
                }
            } else if let Some(target) = topmost_content_at(new_x, new_y) {
                klog_info!("WINDOW_CLICK_FOCUS surface={}", target);
                crate::input_routing::set_focus(target);
            }
        }
        if !left_down {
            DRAGGING = None;
        }
        if let Some((dragging_object, off_x, off_y)) = DRAGGING {
            window_moved = move_window(dragging_object, new_x - off_x, new_y - off_y, fb_width, fb_height);
        }

        if window_moved {
            // The window itself moved -- a real, small cursor-only
            // damage rect could leave a ghost of it at its old
            // position, so this one real case still needs the full
            // recomposite (which also redraws the cursor on top).
            present(fb_phys_base, ppsl, fb_width, fb_height);
        } else {
            // The common case (plain cursor movement, or a click that
            // only changed FOCUS with no visible change): repaint just
            // the real rect the cursor's bitmap occupies at its old
            // AND new position, then draw the cursor at its new spot.
            let pad = 1i32; // real, small margin so the very edge of the bitmap's own pixels never gets clipped by rounding
            let rx = (old_x.min(new_x)) - pad;
            let ry = (old_y.min(new_y)) - pad;
            let rw = (old_x.max(new_x) - old_x.min(new_x)) as u32 + CURSOR_W as u32 + pad as u32 * 2;
            let rh = (old_y.max(new_y) - old_y.min(new_y)) as u32 + CURSOR_H as u32 + pad as u32 * 2;
            redraw_rect(fb_phys_base, ppsl, fb_width, fb_height, rx, ry, rw, rh);
            draw_cursor(fb_phys_base, ppsl, fb_width, fb_height);
        }
    }
}
