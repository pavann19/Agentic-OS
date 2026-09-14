//! Pure Compositor-Driven Animation & Interpolation Logic (Phase 5.14).
//!
//! Provides frame-consistent fixed-point (Q16) easing curves and coordinate interpolation.
//! Decoupled from application rendering: surfaces remain unchanged while the compositor
//! animates positions, transforms, or transitions across frame deadlines.

pub const Q16_ONE: u32 = 65536;

/// Linear interpolation between `start` and `target` using Q16 fixed-point progress [0..65536].
#[inline]
pub fn lerp_i32(start: i32, target: i32, progress_q16: u32) -> i32 {
    let p = progress_q16.min(Q16_ONE) as i64;
    let s = start as i64;
    let t = target as i64;
    let diff = t - s;
    let res = s + (diff * p) / (Q16_ONE as i64);
    res as i32
}

/// Quadratic ease-out interpolation curve: f(p) = p * (2 - p).
/// Fast, organic deceleration without floating-point arithmetic.
#[inline]
pub fn ease_out_quad(progress_q16: u32) -> u32 {
    let p = progress_q16.min(Q16_ONE) as u64;
    // p * (2 * Q16_ONE - p) / Q16_ONE
    let two_one = 2 * (Q16_ONE as u64);
    let factor = two_one.saturating_sub(p);
    ((p * factor) / (Q16_ONE as u64)) as u32
}

/// Calculates animated 2D position for a given start, target, and elapsed time.
pub fn interpolate_position(
    start_x: i32,
    start_y: i32,
    target_x: i32,
    target_y: i32,
    elapsed_cycles: u64,
    duration_cycles: u64,
) -> (i32, i32, bool) {
    if duration_cycles == 0 || elapsed_cycles >= duration_cycles {
        return (target_x, target_y, true); // Complete
    }

    let progress_q16 = ((elapsed_cycles as u128 * Q16_ONE as u128) / duration_cycles as u128) as u32;
    let eased = ease_out_quad(progress_q16);

    let cur_x = lerp_i32(start_x, target_x, eased);
    let cur_y = lerp_i32(start_y, target_y, eased);

    (cur_x, cur_y, false)
}

/// Window slide animation descriptor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SlideAnimation {
    pub start_x: i32,
    pub start_y: i32,
    pub target_x: i32,
    pub target_y: i32,
    pub start_tsc: u64,
    pub duration_cycles: u64,
    pub active: bool,
}

impl SlideAnimation {
    pub const fn new() -> Self {
        Self {
            start_x: 0,
            start_y: 0,
            target_x: 0,
            target_y: 0,
            start_tsc: 0,
            duration_cycles: 0,
            active: false,
        }
    }

    pub fn start(&mut self, start: (i32, i32), target: (i32, i32), now_tsc: u64, duration_cycles: u64) {
        self.start_x = start.0;
        self.start_y = start.1;
        self.target_x = target.0;
        self.target_y = target.1;
        self.start_tsc = now_tsc;
        self.duration_cycles = duration_cycles;
        self.active = true;
    }

    pub fn step(&mut self, now_tsc: u64) -> (i32, i32, bool) {
        if !self.active {
            return (self.target_x, self.target_y, true);
        }

        let elapsed = now_tsc.saturating_sub(self.start_tsc);
        let (x, y, done) = interpolate_position(
            self.start_x,
            self.start_y,
            self.target_x,
            self.target_y,
            elapsed,
            self.duration_cycles,
        );

        if done {
            self.active = false;
        }

        (x, y, done)
    }
}
