//! Damage Region Tracking System (Phase 5.2).
//!
//! Accumulates non-overlapping dirty rectangles per frame without heap allocation.
//! Supports merging overlapping/adjacent rectangles and fallback bounding-box coalescing.

use crate::window_manager::DamageRect;

pub const MAX_DIRTY_RECTS: usize = 16;

#[derive(Copy, Clone, Debug)]
pub struct DamageRegion {
    rects: [DamageRect; MAX_DIRTY_RECTS],
    count: usize,
}

impl DamageRegion {
    pub const fn new() -> Self {
        Self {
            rects: [DamageRect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }; MAX_DIRTY_RECTS],
            count: 0,
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[inline(always)]
    pub fn count(&self) -> usize {
        self.count
    }

    #[inline(always)]
    pub fn rects(&self) -> &[DamageRect] {
        &self.rects[..self.count]
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Add a dirty rectangle to the damage region, coalescing with existing rects if overlapping or adjacent.
    pub fn add_rect(&mut self, mut new_rect: DamageRect) {
        if new_rect.width == 0 || new_rect.height == 0 {
            return;
        }

        // Try merging with any overlapping or adjacent rectangle
        let mut i = 0;
        while i < self.count {
            if rects_overlap_or_adjacent(&self.rects[i], &new_rect) {
                new_rect = union_rect(&self.rects[i], &new_rect);
                // Remove rect i and re-test merged rect against remaining
                self.rects.swap(i, self.count - 1);
                self.count -= 1;
                i = 0;
                continue;
            }
            i += 1;
        }

        if self.count < MAX_DIRTY_RECTS {
            self.rects[self.count] = new_rect;
            self.count += 1;
        } else {
            // Threshold exceeded (> 16 rects): coalesce all rects into a single bounding union
            let mut bounding = new_rect;
            for r in &self.rects[..self.count] {
                bounding = union_rect(&bounding, r);
            }
            self.rects[0] = bounding;
            self.count = 1;
        }
    }

    pub fn total_pixels(&self) -> usize {
        self.rects[..self.count]
            .iter()
            .map(|r| (r.width as usize) * (r.height as usize))
            .sum()
    }
}

#[inline(always)]
pub fn rects_overlap_or_adjacent(a: &DamageRect, b: &DamageRect) -> bool {
    let a_x1 = a.x + a.width as i32;
    let a_y1 = a.y + a.height as i32;
    let b_x1 = b.x + b.width as i32;
    let b_y1 = b.y + b.height as i32;

    // Bounding check including 1px adjacency
    !(b.x > a_x1 || b_x1 < a.x || b.y > a_y1 || b_y1 < a.y)
}

#[inline(always)]
pub fn union_rect(a: &DamageRect, b: &DamageRect) -> DamageRect {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.width as i32).max(b.x + b.width as i32);
    let y1 = (a.y + a.height as i32).max(b.y + b.height as i32);
    DamageRect {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0) as u32,
        height: (y1 - y0).max(0) as u32,
    }
}
