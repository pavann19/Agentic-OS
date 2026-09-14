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

/// Display timing and presentation synchronization (Phase 5.12).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DisplayTiming {
    pub refresh_rate_hz: u32,
    pub last_vsync_tsc: u64,
    pub total_vsync_events: u64,
}

impl DisplayTiming {
    pub const fn new(refresh_rate_hz: u32) -> Self {
        Self {
            refresh_rate_hz,
            last_vsync_tsc: 0,
            total_vsync_events: 0,
        }
    }

    #[inline]
    pub fn refresh_period_us(&self) -> u64 {
        if self.refresh_rate_hz == 0 {
            0
        } else {
            1_000_000u64 / self.refresh_rate_hz as u64
        }
    }

    #[inline]
    pub fn refresh_period_cycles(&self, cycles_per_us: u64) -> u64 {
        self.refresh_period_us() * cycles_per_us
    }

    /// Calculates the next expected display refresh deadline aligned to periodic display cadence.
    pub fn next_deadline_tsc(&self, now_tsc: u64, cycles_per_us: u64) -> u64 {
        let period = self.refresh_period_cycles(cycles_per_us);
        if period == 0 || self.last_vsync_tsc == 0 {
            return now_tsc;
        }
        let elapsed = now_tsc.saturating_sub(self.last_vsync_tsc);
        let periods_elapsed = elapsed / period;
        self.last_vsync_tsc + (periods_elapsed + 1) * period
    }

    /// Records a VSync or presentation synchronization event.
    pub fn record_vsync(&mut self, now_tsc: u64) {
        self.last_vsync_tsc = now_tsc;
        self.total_vsync_events += 1;
    }
}

/// Presentation fence sequence tracker for non-blocking client/compositor synchronization (Phase 5.12).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PresentationFence {
    pub sequence: u64,
}

impl PresentationFence {
    pub const fn new() -> Self {
        Self { sequence: 0 }
    }

    #[inline]
    pub fn advance(&mut self) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        self.sequence
    }

    #[inline]
    pub fn is_reached(&self, target: u64) -> bool {
        self.sequence >= target
    }
}

