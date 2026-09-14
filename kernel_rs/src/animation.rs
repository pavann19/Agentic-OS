//! Compositor Animation Engine (Phase 5.14).
//!
//! Provides compositor-driven visual transitions, smooth window movements,
//! and transform interpolation without requiring applications to re-render.

use core::sync::atomic::{AtomicBool, Ordering};
use kernel_common::animation::SlideAnimation;
use crate::capability::ObjectId;
use crate::compositor_metrics::CYCLES_PER_US;

const MAX_ANIMATIONS: usize = 16;

#[derive(Copy, Clone, Debug)]
struct ActiveAnimation {
    surface_object: ObjectId,
    slide: SlideAnimation,
}

impl ActiveAnimation {
    const fn empty() -> Self {
        Self {
            surface_object: 0,
            slide: SlideAnimation::new(),
        }
    }
}

static HAS_ACTIVE: AtomicBool = AtomicBool::new(false);
static mut ANIMATIONS: [ActiveAnimation; MAX_ANIMATIONS] = [ActiveAnimation::empty(); MAX_ANIMATIONS];

/// Starts a smooth slide animation for `surface_object` towards `(target_x, target_y)`.
pub fn start_window_slide(surface_object: ObjectId, current_pos: (i32, i32), target_pos: (i32, i32), duration_ms: u64) {
    let now = crate::compositor_metrics::read_tsc();
    let duration_cycles = duration_ms * 1000 * CYCLES_PER_US;

    unsafe {
        let anims = &mut *&raw mut ANIMATIONS;
        let mut target_idx = None;
        for i in 0..MAX_ANIMATIONS {
            if anims[i].slide.active && anims[i].surface_object == surface_object {
                target_idx = Some(i);
                break;
            }
        }
        if target_idx.is_none() {
            for i in 0..MAX_ANIMATIONS {
                if !anims[i].slide.active {
                    target_idx = Some(i);
                    break;
                }
            }
        }
        if let Some(i) = target_idx {
            anims[i].surface_object = surface_object;
            anims[i].slide.start(current_pos, target_pos, now, duration_cycles);
            HAS_ACTIVE.store(true, Ordering::Release);
        }
    }
}

/// Steps all active compositor animations for the current frame tick.
/// Returns true if any animation is still running.
pub fn step_animations() -> bool {
    if !HAS_ACTIVE.load(Ordering::Acquire) {
        return false;
    }

    let Some((_fb_phys, _ppsl, fb_width, fb_height)) = crate::compositor::get_fb_params() else {
        return false;
    };

    let now = crate::compositor_metrics::read_tsc();
    let mut any_active = false;

    unsafe {
        let anims = &mut *&raw mut ANIMATIONS;
        for a in anims.iter_mut() {
            if a.slide.active {
                let (new_x, new_y, done) = a.slide.step(now);
                crate::window_manager::move_window(a.surface_object, new_x, new_y, fb_width, fb_height);

                if !done {
                    any_active = true;
                }
            }
        }
    }

    HAS_ACTIVE.store(any_active, Ordering::Release);
    any_active
}

/// Returns true if any compositor animation is currently in flight.
#[inline(always)]
pub fn has_active_animations() -> bool {
    HAS_ACTIVE.load(Ordering::Acquire)
}
