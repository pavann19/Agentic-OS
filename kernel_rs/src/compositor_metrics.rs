//! Compositor Performance Telemetry & Measurement Infrastructure (Phase 5.0).
//!
//! Provides lightweight, zero-allocation cycle-accurate performance counters
//! using the x86 `_rdtsc` instruction. Tracks:
//!  - Frame composition time, damage calculation time, framebuffer flush time
//!  - Cursor update time and input event latency
//!  - Damaged rectangles, scanlines, and pixel counts
//!  - Rendered, presented, and dropped frame counts
//!  - Input throughput (mouse packets/sec, keyboard events/sec)

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

pub const CYCLES_PER_US: u64 = 2500; // ~2.5 GHz nominal clock on modern x86 / QEMU

#[derive(Copy, Clone, Default, Debug)]
pub struct FrameMetrics {
    pub frame_time_cycles: u64,
    pub input_time_cycles: u64,
    pub damage_time_cycles: u64,
    pub compose_time_cycles: u64,
    pub flush_time_cycles: u64,
    pub cursor_time_cycles: u64,
    pub damaged_rects: usize,
    pub damaged_scanlines: usize,
    pub damaged_pixels: usize,
}

impl FrameMetrics {
    #[inline(always)]
    pub fn frame_time_us(&self) -> u64 {
        self.frame_time_cycles / CYCLES_PER_US
    }
    #[inline(always)]
    pub fn input_time_us(&self) -> u64 {
        self.input_time_cycles / CYCLES_PER_US
    }
    #[inline(always)]
    pub fn damage_time_us(&self) -> u64 {
        self.damage_time_cycles / CYCLES_PER_US
    }
    #[inline(always)]
    pub fn compose_time_us(&self) -> u64 {
        self.compose_time_cycles / CYCLES_PER_US
    }
    #[inline(always)]
    pub fn flush_time_us(&self) -> u64 {
        self.flush_time_cycles / CYCLES_PER_US
    }
    #[inline(always)]
    pub fn cursor_time_us(&self) -> u64 {
        self.cursor_time_cycles / CYCLES_PER_US
    }
}

static FRAMES_RENDERED: AtomicU64 = AtomicU64::new(0);
static FRAMES_PRESENTED: AtomicU64 = AtomicU64::new(0);
static FRAMES_DROPPED: AtomicU64 = AtomicU64::new(0);
static MOUSE_EVENTS: AtomicU64 = AtomicU64::new(0);
static KEYBOARD_EVENTS: AtomicU64 = AtomicU64::new(0);

static ACC_FRAME_CYCLES: AtomicU64 = AtomicU64::new(0);
static ACC_INPUT_CYCLES: AtomicU64 = AtomicU64::new(0);
static ACC_DAMAGE_CYCLES: AtomicU64 = AtomicU64::new(0);
static ACC_COMPOSE_CYCLES: AtomicU64 = AtomicU64::new(0);
static ACC_FLUSH_CYCLES: AtomicU64 = AtomicU64::new(0);
static ACC_CURSOR_CYCLES: AtomicU64 = AtomicU64::new(0);
static ACC_DAMAGED_RECTS: AtomicUsize = AtomicUsize::new(0);
static ACC_DAMAGED_SCANLINES: AtomicUsize = AtomicUsize::new(0);
static ACC_DAMAGED_PIXELS: AtomicUsize = AtomicUsize::new(0);
static SAMPLE_FRAMES: AtomicU64 = AtomicU64::new(0);

static LAST_REPORT_FRAME: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
pub fn read_tsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[inline(always)]
pub fn record_mouse_event() {
    MOUSE_EVENTS.fetch_add(1, Ordering::Relaxed);
}

#[inline(always)]
pub fn record_keyboard_event() {
    KEYBOARD_EVENTS.fetch_add(1, Ordering::Relaxed);
}

#[inline(always)]
pub fn record_dropped_frame() {
    FRAMES_DROPPED.fetch_add(1, Ordering::Relaxed);
}

pub fn record_frame(metrics: &FrameMetrics) {
    FRAMES_RENDERED.fetch_add(1, Ordering::Relaxed);
    FRAMES_PRESENTED.fetch_add(1, Ordering::Relaxed);
    SAMPLE_FRAMES.fetch_add(1, Ordering::Relaxed);

    ACC_FRAME_CYCLES.fetch_add(metrics.frame_time_cycles, Ordering::Relaxed);
    ACC_INPUT_CYCLES.fetch_add(metrics.input_time_cycles, Ordering::Relaxed);
    ACC_DAMAGE_CYCLES.fetch_add(metrics.damage_time_cycles, Ordering::Relaxed);
    ACC_COMPOSE_CYCLES.fetch_add(metrics.compose_time_cycles, Ordering::Relaxed);
    ACC_FLUSH_CYCLES.fetch_add(metrics.flush_time_cycles, Ordering::Relaxed);
    ACC_CURSOR_CYCLES.fetch_add(metrics.cursor_time_cycles, Ordering::Relaxed);
    ACC_DAMAGED_RECTS.fetch_add(metrics.damaged_rects, Ordering::Relaxed);
    ACC_DAMAGED_SCANLINES.fetch_add(metrics.damaged_scanlines, Ordering::Relaxed);
    ACC_DAMAGED_PIXELS.fetch_add(metrics.damaged_pixels, Ordering::Relaxed);

    // Periodically log metrics every 60 frames
    let total = FRAMES_PRESENTED.load(Ordering::Relaxed);
    let last = LAST_REPORT_FRAME.load(Ordering::Relaxed);
    if total.saturating_sub(last) >= 60 {
        LAST_REPORT_FRAME.store(total, Ordering::Relaxed);
        log_sample_metrics(false);
    }
}

pub fn log_sample_metrics(reset: bool) {
    let count = SAMPLE_FRAMES.load(Ordering::Relaxed);
    if count == 0 {
        return;
    }
    let frame_us = (ACC_FRAME_CYCLES.load(Ordering::Relaxed) / count) / CYCLES_PER_US;
    let input_us = (ACC_INPUT_CYCLES.load(Ordering::Relaxed) / count) / CYCLES_PER_US;
    let damage_us = (ACC_DAMAGE_CYCLES.load(Ordering::Relaxed) / count) / CYCLES_PER_US;
    let compose_us = (ACC_COMPOSE_CYCLES.load(Ordering::Relaxed) / count) / CYCLES_PER_US;
    let flush_us = (ACC_FLUSH_CYCLES.load(Ordering::Relaxed) / count) / CYCLES_PER_US;
    let cursor_us = (ACC_CURSOR_CYCLES.load(Ordering::Relaxed) / count) / CYCLES_PER_US;
    let rects = ACC_DAMAGED_RECTS.load(Ordering::Relaxed) / (count as usize);
    let scanlines = ACC_DAMAGED_SCANLINES.load(Ordering::Relaxed) / (count as usize);
    let pixels = ACC_DAMAGED_PIXELS.load(Ordering::Relaxed) / (count as usize);
    let mouse = MOUSE_EVENTS.load(Ordering::Relaxed);
    let keys = KEYBOARD_EVENTS.load(Ordering::Relaxed);
    let dropped = FRAMES_DROPPED.load(Ordering::Relaxed);
    let total_frames = FRAMES_PRESENTED.load(Ordering::Relaxed);

    crate::klog_info!(
        "[GUI_METRICS] frames={} sample_count={} frame_us={} compose_us={} flush_us={} cursor_us={} input_us={} damage_us={} damaged_rects={} damaged_scanlines={} damaged_pixels={} mouse_events={} key_events={} dropped={}",
        total_frames, count, frame_us, compose_us, flush_us, cursor_us, input_us, damage_us, rects, scanlines, pixels, mouse, keys, dropped
    );

    if reset {
        reset_metrics();
    }
}

pub fn dump_summary(scenario: &str) {
    let count = SAMPLE_FRAMES.load(Ordering::Relaxed);
    let safe_count = count.max(1);
    let frame_us = (ACC_FRAME_CYCLES.load(Ordering::Relaxed) / safe_count) / CYCLES_PER_US;
    let input_us = (ACC_INPUT_CYCLES.load(Ordering::Relaxed) / safe_count) / CYCLES_PER_US;
    let damage_us = (ACC_DAMAGE_CYCLES.load(Ordering::Relaxed) / safe_count) / CYCLES_PER_US;
    let compose_us = (ACC_COMPOSE_CYCLES.load(Ordering::Relaxed) / safe_count) / CYCLES_PER_US;
    let flush_us = (ACC_FLUSH_CYCLES.load(Ordering::Relaxed) / safe_count) / CYCLES_PER_US;
    let cursor_us = (ACC_CURSOR_CYCLES.load(Ordering::Relaxed) / safe_count) / CYCLES_PER_US;
    let rects = ACC_DAMAGED_RECTS.load(Ordering::Relaxed) / (safe_count as usize);
    let scanlines = ACC_DAMAGED_SCANLINES.load(Ordering::Relaxed) / (safe_count as usize);
    let pixels = ACC_DAMAGED_PIXELS.load(Ordering::Relaxed) / (safe_count as usize);
    let mouse = MOUSE_EVENTS.load(Ordering::Relaxed);
    let keys = KEYBOARD_EVENTS.load(Ordering::Relaxed);
    let dropped = FRAMES_DROPPED.load(Ordering::Relaxed);
    let total_frames = FRAMES_PRESENTED.load(Ordering::Relaxed);

    crate::klog_info!(
        "[GUI_BASELINE_SUMMARY] scenario={} total_frames={} sample_count={} avg_frame_us={} avg_compose_us={} avg_flush_us={} avg_cursor_us={} avg_input_us={} avg_damage_us={} avg_damaged_rects={} avg_damaged_scanlines={} avg_damaged_pixels={} mouse_events={} key_events={} dropped={}",
        scenario, total_frames, count, frame_us, compose_us, flush_us, cursor_us, input_us, damage_us, rects, scanlines, pixels, mouse, keys, dropped
    );
    reset_metrics();
}

pub fn reset_metrics() {
    ACC_FRAME_CYCLES.store(0, Ordering::Relaxed);
    ACC_INPUT_CYCLES.store(0, Ordering::Relaxed);
    ACC_DAMAGE_CYCLES.store(0, Ordering::Relaxed);
    ACC_COMPOSE_CYCLES.store(0, Ordering::Relaxed);
    ACC_FLUSH_CYCLES.store(0, Ordering::Relaxed);
    ACC_CURSOR_CYCLES.store(0, Ordering::Relaxed);
    ACC_DAMAGED_RECTS.store(0, Ordering::Relaxed);
    ACC_DAMAGED_SCANLINES.store(0, Ordering::Relaxed);
    ACC_DAMAGED_PIXELS.store(0, Ordering::Relaxed);
    SAMPLE_FRAMES.store(0, Ordering::Relaxed);
}
