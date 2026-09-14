//! Pure geometric types and algorithms for clipping and occlusion (Phase 5.4).
//!
//! Provides non-allocating rectangle intersection, difference/subtraction,
//! containment, and visibility pipeline for overlapping windows.

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[inline(always)]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    #[inline(always)]
    pub const fn empty() -> Self {
        Self { x: 0, y: 0, width: 0, height: 0 }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    #[inline(always)]
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    #[inline(always)]
    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    #[inline(always)]
    pub fn area(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// Computes the intersection of `self` and `other`. Returns `None` if they do not overlap.
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());

        if x1 > x0 && y1 > y0 {
            Some(Rect {
                x: x0,
                y: y0,
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
            })
        } else {
            None
        }
    }

    /// Returns true if `self` completely contains `other`.
    #[inline(always)]
    pub fn contains_rect(&self, other: &Rect) -> bool {
        if other.is_empty() {
            return true;
        }
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }

    /// Subtracts `occluder` from `self` (`self - occluder`).
    ///
    /// Returns 0 to 4 disjoint sub-rectangles written into `out`.
    /// Returns the number of sub-rectangles generated.
    pub fn subtract(&self, occluder: &Rect, out: &mut [Rect; 4]) -> usize {
        if self.is_empty() {
            return 0;
        }
        let Some(inter) = self.intersect(occluder) else {
            out[0] = *self;
            return 1;
        };

        if occluder.contains_rect(self) {
            // Completely occluded
            return 0;
        }

        let mut count = 0;

        // 1. Top slice: span above intersection across full width of self
        if inter.y > self.y {
            out[count] = Rect {
                x: self.x,
                y: self.y,
                width: self.width,
                height: (inter.y - self.y) as u32,
            };
            count += 1;
        }

        // 2. Bottom slice: span below intersection across full width of self
        if self.bottom() > inter.bottom() {
            out[count] = Rect {
                x: self.x,
                y: inter.bottom(),
                width: self.width,
                height: (self.bottom() - inter.bottom()) as u32,
            };
            count += 1;
        }

        // 3. Left slice: span between inter.y and inter.bottom() to the left of inter
        if inter.x > self.x {
            out[count] = Rect {
                x: self.x,
                y: inter.y,
                width: (inter.x - self.x) as u32,
                height: inter.height,
            };
            count += 1;
        }

        // 4. Right slice: span between inter.y and inter.bottom() to the right of inter
        if self.right() > inter.right() {
            out[count] = Rect {
                x: inter.right(),
                y: inter.y,
                width: (self.right() - inter.right()) as u32,
                height: inter.height,
            };
            count += 1;
        }

        count
    }

    /// Computes the visible non-overlapping sub-rectangles of `target` after subtracting
    /// all `occluders` (higher z-order windows).
    ///
    /// Results are written into `out` (capacity at least 32). Returns number of visible rects.
    pub fn compute_visible_rects(target: &Rect, occluders: &[Rect], out: &mut [Rect; 32]) -> usize {
        if target.is_empty() {
            return 0;
        }
        let mut current = [Rect::default(); 32];
        let mut current_len = 1;
        current[0] = *target;

        for occ in occluders {
            if occ.is_empty() {
                continue;
            }
            if current_len == 0 {
                break;
            }
            let mut next = [Rect::default(); 32];
            let mut next_len = 0;

            for i in 0..current_len {
                let mut sub = [Rect::default(); 4];
                let sub_len = current[i].subtract(occ, &mut sub);
                for k in 0..sub_len {
                    if next_len < 32 {
                        next[next_len] = sub[k];
                        next_len += 1;
                    }
                }
            }
            current = next;
            current_len = next_len;
        }

        for i in 0..current_len {
            out[i] = current[i];
        }
        current_len
    }
}
