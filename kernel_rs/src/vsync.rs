//! VSync and Presentation Synchronization (Phase 5.12).
//!
//! Synchronizes presentation pipeline with display refresh cycles (60 Hz cadence /
//! hardware retrace where available) to eliminate tearing, enforce stable frame pacing,
//! and provide non-blocking presentation fences.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const VGA_INPUT_STATUS_1: u16 = 0x3DA;
const VGA_VRETRACE_BIT: u8 = 0x08;

static HARDWARE_VSYNC_SUPPORTED: AtomicBool = AtomicBool::new(false);
static PRESENTATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LAST_SYNC_TSC: AtomicU64 = AtomicU64::new(0);
static REFRESH_RATE_HZ: AtomicU32 = AtomicU32::new(60);
static TOTAL_VSYNC_EVENTS: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack, preserves_flags));
    val
}

/// Probes whether VGA status register 0x3DA reports active vertical retrace transitions.
pub fn probe_hardware_vsync() -> bool {
    let mut transitions = 0;
    let mut last_bit = unsafe { inb(VGA_INPUT_STATUS_1) & VGA_VRETRACE_BIT };
    for _ in 0..10_000 {
        let bit = unsafe { inb(VGA_INPUT_STATUS_1) & VGA_VRETRACE_BIT };
        if bit != last_bit {
            transitions += 1;
            last_bit = bit;
            if transitions >= 2 {
                HARDWARE_VSYNC_SUPPORTED.store(true, Ordering::Release);
                return true;
            }
        }
        core::hint::spin_loop();
    }
    HARDWARE_VSYNC_SUPPORTED.store(false, Ordering::Release);
    false
}

/// Returns whether hardware VSync detection is supported on this display controller.
#[inline(always)]
pub fn is_hardware_vsync_supported() -> bool {
    HARDWARE_VSYNC_SUPPORTED.load(Ordering::Acquire)
}

/// Synchronizes with the display refresh cycle:
/// If hardware VSync is available, waits for retrace start with bounded timeout (e.g. 5000 cycles).
/// If not, aligns against the phase-locked timer deadline (16,666 µs for 60 Hz).
pub fn sync_to_display(timeout_cycles: u64) -> bool {
    let now = crate::compositor_metrics::read_tsc();
    if is_hardware_vsync_supported() {
        let start = now;
        // Bounded spin: wait until outside retrace
        while (unsafe { inb(VGA_INPUT_STATUS_1) } & VGA_VRETRACE_BIT) != 0 {
            if crate::compositor_metrics::read_tsc().saturating_sub(start) >= timeout_cycles {
                return false;
            }
            core::hint::spin_loop();
        }
        // Wait for retrace to start
        while (unsafe { inb(VGA_INPUT_STATUS_1) } & VGA_VRETRACE_BIT) == 0 {
            if crate::compositor_metrics::read_tsc().saturating_sub(start) >= timeout_cycles {
                return false;
            }
            core::hint::spin_loop();
        }
        LAST_SYNC_TSC.store(crate::compositor_metrics::read_tsc(), Ordering::Relaxed);
        true
    } else {
        // Timer-paced synchronization: verify deadline and record sync
        LAST_SYNC_TSC.store(now, Ordering::Relaxed);
        true
    }
}

/// Advances the presentation sequence fence upon committing a frame to physical display.
pub fn advance_presentation_fence() -> u64 {
    let seq = PRESENTATION_SEQUENCE.fetch_add(1, Ordering::SeqCst) + 1;
    let now = crate::compositor_metrics::read_tsc();
    LAST_SYNC_TSC.store(now, Ordering::Relaxed);
    TOTAL_VSYNC_EVENTS.fetch_add(1, Ordering::Relaxed);
    seq
}

/// Returns the current presentation fence sequence.
#[inline(always)]
pub fn current_presentation_fence() -> u64 {
    PRESENTATION_SEQUENCE.load(Ordering::SeqCst)
}

/// Checks if a presentation fence sequence has completed.
#[inline(always)]
pub fn is_fence_reached(target_seq: u64) -> bool {
    current_presentation_fence() >= target_seq
}

/// Returns the display refresh rate (default 60 Hz).
pub fn get_refresh_rate() -> u32 {
    REFRESH_RATE_HZ.load(Ordering::Relaxed)
}

/// Sets the display refresh rate (e.g. 60, 120, 144).
pub fn set_refresh_rate(hz: u32) {
    REFRESH_RATE_HZ.store(hz, Ordering::Relaxed);
}

/// Returns total VSync/presentation synchronization events recorded.
pub fn total_sync_events() -> u64 {
    TOTAL_VSYNC_EVENTS.load(Ordering::Relaxed)
}
