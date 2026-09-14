//! Frame Scheduler for Modern Compositor Architecture (Phase 5.7).
//!
//! Decouples input frequency (e.g. 1000 Hz mouse) from rendering presentation frequency.
//! Gated by target frame rates (e.g. 60 FPS = ~16.666 ms deadline), avoiding redundant
//! desktop composition passes when events arrive faster than the display refresh deadline.
//!
//! Characteristics:
//!  - Zero heap allocation: atomic state tracking for target FPS, timestamps, and pending status.
//!  - Frame deadlines and frame pacing using TSC (`compositor_metrics::read_tsc()`).
//!  - Dirty-driven: renders ONLY when damage exists AND frame deadline has elapsed.
//!  - Zero CPU usage when the desktop is completely idle.
//!  - Force presentation support for drag-release and critical state transitions.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use crate::compositor_metrics::CYCLES_PER_US;

pub const DEFAULT_TARGET_FPS: u32 = 60;

static TARGET_FPS: AtomicU32 = AtomicU32::new(DEFAULT_TARGET_FPS);
static LAST_PRESENT_TSC: AtomicU64 = AtomicU64::new(0);
static FRAME_PENDING: AtomicBool = AtomicBool::new(false);

static FRAMES_REQUESTED: AtomicU64 = AtomicU64::new(0);
static FRAMES_PRESENTED: AtomicU64 = AtomicU64::new(0);
static FRAMES_THROTTLED: AtomicU64 = AtomicU64::new(0);

/// Sets the compositor's target FPS (e.g. 60, 120, 144, or 0 for unthrottled).
pub fn set_target_fps(fps: u32) {
    TARGET_FPS.store(fps, Ordering::Relaxed);
}

/// Returns the current target FPS.
pub fn get_target_fps() -> u32 {
    TARGET_FPS.load(Ordering::Relaxed)
}

/// Calculates the frame deadline in CPU cycles based on target FPS.
#[inline(always)]
pub fn frame_interval_cycles() -> u64 {
    let fps = TARGET_FPS.load(Ordering::Relaxed);
    if fps == 0 {
        return 0; // Unthrottled / immediate
    }
    (1_000_000u64 / fps as u64) * CYCLES_PER_US
}

/// Checks if a frame may be presented given the current TSC and last presentation timestamp.
#[inline(always)]
pub fn can_present(now: u64) -> bool {
    let last = LAST_PRESENT_TSC.load(Ordering::Relaxed);
    if last == 0 {
        return true;
    }
    let interval = frame_interval_cycles();
    now.saturating_sub(last) >= interval
}

/// Returns whether a deferred frame is pending presentation.
#[inline(always)]
pub fn is_frame_pending() -> bool {
    FRAME_PENDING.load(Ordering::Acquire)
}

/// Requests a frame presentation through the frame scheduler.
///
/// If `force` is true, or if enough time has elapsed since the previous presentation (`can_present`),
/// the pending damage is immediately flushed via `window_manager::flush_dirty_surfaces`.
/// Otherwise, presentation is deferred and coalesced into the next frame deadline, saving CPU
/// cycles and preventing 1000 Hz re-composition during high-frequency mouse dragging.
pub unsafe fn request_presentation(
    fb_phys_base: u64,
    ppsl: u32,
    fb_width: u32,
    fb_height: u32,
    force: bool,
) -> bool {
    FRAMES_REQUESTED.fetch_add(1, Ordering::Relaxed);
    let now = crate::compositor_metrics::read_tsc();

    if force || can_present(now) {
        // Synchronize with display refresh cycle (Phase 5.12)
        crate::vsync::sync_to_display(5_000 * CYCLES_PER_US);

        LAST_PRESENT_TSC.store(now, Ordering::Relaxed);
        FRAME_PENDING.store(false, Ordering::Release);
        FRAMES_PRESENTED.fetch_add(1, Ordering::Relaxed);
        crate::window_manager::flush_dirty_surfaces(fb_phys_base, ppsl, fb_width, fb_height);
        crate::vsync::advance_presentation_fence();
        true
    } else {
        // Throttle this frame: accumulate damage in window manager and defer to next deadline
        FRAME_PENDING.store(true, Ordering::Release);
        FRAMES_THROTTLED.fetch_add(1, Ordering::Relaxed);
        crate::compositor_metrics::record_dropped_frame();
        false
    }
}

/// Checks if a deferred frame or compositor animation is pending and presents it if the frame deadline has now passed.
pub unsafe fn try_present_pending(
    fb_phys_base: u64,
    ppsl: u32,
    fb_width: u32,
    fb_height: u32,
) -> bool {
    let anim_active = crate::animation::has_active_animations();
    if !is_frame_pending() && !anim_active {
        return false;
    }
    let now = crate::compositor_metrics::read_tsc();
    if can_present(now) {
        if anim_active {
            crate::animation::step_animations();
        }
        request_presentation(fb_phys_base, ppsl, fb_width, fb_height, true)
    } else {
        false
    }
}

/// Returns (requested, presented, throttled) frame statistics.
pub fn get_stats() -> (u64, u64, u64) {
    (
        FRAMES_REQUESTED.load(Ordering::Relaxed),
        FRAMES_PRESENTED.load(Ordering::Relaxed),
        FRAMES_THROTTLED.load(Ordering::Relaxed),
    )
}

/// Resets frame scheduler telemetry.
pub fn reset_stats() {
    FRAMES_REQUESTED.store(0, Ordering::Relaxed);
    FRAMES_PRESENTED.store(0, Ordering::Relaxed);
    FRAMES_THROTTLED.store(0, Ordering::Relaxed);
    FRAME_PENDING.store(false, Ordering::Relaxed);
    LAST_PRESENT_TSC.store(0, Ordering::Relaxed);
}
