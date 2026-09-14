//! Compositor Renderer Abstraction (Phase 5.11).
//!
//! Provides a clean hardware abstraction layer separating composition policy
//! (window layout, z-ordering, damage tracking, and frame scheduling)
//! from rendering execution (CPU scanline blitting vs future GPU acceleration).

use crate::window_manager::{put_fb_scanline_fast, DamageRect};

pub trait Renderer {
    /// Clears a rectangular region in the offscreen buffer to a solid color.
    fn clear_rect(&mut self, buf: &mut [u32], buf_width: u32, buf_height: u32, rect: DamageRect, color: u32);

    /// Blits a surface's visible content into the offscreen buffer after clipping against occluders and a clip rectangle.
    fn blit_surface_clipped(
        &mut self,
        bb: &mut [u32],
        fb_width: u32,
        fb_height: u32,
        src_buf: &[u32],
        src_w: u32,
        src_h: u32,
        dst_x: i32,
        dst_y: i32,
        clip_rect: DamageRect,
        occluders: &[DamageRect],
    );

    /// Synchronizes a damaged rectangle from BACKBUFFER to FRONTBUFFER and flushes scanlines to display MMIO.
    unsafe fn present_rect(
        &mut self,
        fb_phys_base: u64,
        ppsl: u32,
        fb_width: u32,
        fb_height: u32,
        bb: &[u32],
        fb: &mut [u32],
        rect: DamageRect,
    ) -> usize;
}

/// High-performance CPU-based renderer utilizing RAM scanlines and Write-Combining GOP blits.
pub struct CpuRenderer;

impl Renderer for CpuRenderer {
    fn clear_rect(&mut self, buf: &mut [u32], buf_width: u32, buf_height: u32, rect: DamageRect, color: u32) {
        let x0 = rect.x.max(0) as u32;
        let y0 = rect.y.max(0) as u32;
        let x1 = (rect.x + rect.width as i32).clamp(0, buf_width as i32) as u32;
        let y1 = (rect.y + rect.height as i32).clamp(0, buf_height as i32) as u32;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let count = (x1 - x0) as usize;
        for y in y0..y1 {
            let row_off = (y * buf_width + x0) as usize;
            buf[row_off..row_off + count].fill(color);
        }
    }

    fn blit_surface_clipped(
        &mut self,
        bb: &mut [u32],
        fb_width: u32,
        fb_height: u32,
        src_buf: &[u32],
        src_w: u32,
        src_h: u32,
        dst_x: i32,
        dst_y: i32,
        clip_rect: DamageRect,
        occluders: &[DamageRect],
    ) {
        let full_target = DamageRect::new(dst_x, dst_y, src_w, src_h);
        let Some(target_rect) = full_target.intersect(&clip_rect) else {
            return;
        };
        let screen_rect = DamageRect::new(0, 0, fb_width, fb_height);
        let mut vis = [DamageRect::default(); 32];
        let n = DamageRect::compute_visible_rects(&target_rect, occluders, &mut vis);

        for i in 0..n {
            if let Some(rc) = vis[i].intersect(&screen_rect) {
                let slice_w = rc.width as usize;
                for sy in rc.y..rc.bottom() {
                    let local_y = (sy - dst_y) as u32;
                    let local_x = (rc.x - dst_x) as u32;
                    let src_off = (local_y * src_w + local_x) as usize;
                    let dst_off = (sy as u32 * fb_width + rc.x as u32) as usize;
                    bb[dst_off..dst_off + slice_w].copy_from_slice(&src_buf[src_off..src_off + slice_w]);
                }
            }
        }
    }

    unsafe fn present_rect(
        &mut self,
        fb_phys_base: u64,
        ppsl: u32,
        fb_width: u32,
        fb_height: u32,
        bb: &[u32],
        fb: &mut [u32],
        rect: DamageRect,
    ) -> usize {
        let x0 = rect.x.max(0) as u32;
        let y0 = rect.y.max(0) as u32;
        let x1 = (rect.x + rect.width as i32).clamp(0, fb_width as i32) as u32;
        let y1 = (rect.y + rect.height as i32).clamp(0, fb_height as i32) as u32;
        if x0 >= x1 || y0 >= y1 {
            return 0;
        }
        let count = (x1 - x0) as usize;
        let mut scanlines = 0;
        for y in y0..y1 {
            let offset = (y * fb_width + x0) as usize;
            fb[offset..offset + count].copy_from_slice(&bb[offset..offset + count]);
            let src_ptr = fb.as_ptr().add(offset);
            put_fb_scanline_fast(fb_phys_base, ppsl, x0, y, src_ptr, count as u32);
            scanlines += 1;
        }
        core::arch::asm!("sfence", options(nomem, nostack));
        scanlines
    }
}

pub static mut CPU_RENDERER: CpuRenderer = CpuRenderer;

#[inline(always)]
pub fn active_renderer() -> &'static mut CpuRenderer {
    unsafe { &mut *&raw mut CPU_RENDERER }
}
