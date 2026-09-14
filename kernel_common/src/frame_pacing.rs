//! Pure frame pacing and scheduling logic (Phase 5.7).
//!
//! Provides hardware-independent frame deadline, interval calculation,
//! and dirty-driven presentation state gating.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FramePacer {
    pub target_fps: u32,
    pub last_present_tsc: u64,
    pub has_damage: bool,
    pub dropped_frames: u64,
    pub presented_frames: u64,
}

impl FramePacer {
    pub const fn new(target_fps: u32) -> Self {
        Self {
            target_fps,
            last_present_tsc: 0,
            has_damage: false,
            dropped_frames: 0,
            presented_frames: 0,
        }
    }

    #[inline]
    pub fn frame_interval_us(&self) -> u64 {
        if self.target_fps == 0 {
            0
        } else {
            1_000_000u64 / self.target_fps as u64
        }
    }

    #[inline]
    pub fn frame_interval_cycles(&self, cycles_per_us: u64) -> u64 {
        self.frame_interval_us() * cycles_per_us
    }

    #[inline]
    pub fn mark_damage(&mut self) {
        self.has_damage = true;
    }

    /// Evaluates whether a frame should be presented at `now_tsc`.
    /// Returns `true` if damage is pending AND the frame deadline has elapsed (or `force` is true).
    pub fn request_presentation(&mut self, now_tsc: u64, cycles_per_us: u64, force: bool) -> bool {
        if !self.has_damage && !force {
            return false;
        }

        let interval = self.frame_interval_cycles(cycles_per_us);
        let elapsed = now_tsc.saturating_sub(self.last_present_tsc);

        if force || self.last_present_tsc == 0 || elapsed >= interval {
            self.last_present_tsc = now_tsc;
            self.has_damage = false;
            self.presented_frames += 1;
            true
        } else {
            self.dropped_frames += 1;
            false
        }
    }
}
