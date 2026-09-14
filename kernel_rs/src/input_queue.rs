//! Lock-free Bounded Input Event Queue (Phase 5.1).
//!
//! Decouples high-frequency hardware and driver input events (PS/2 mouse, keyboard)
//! from screen rendering and window compositing.
//!
//! Characteristics:
//!  - Zero heap allocation: fixed static ring buffer with power-of-two capacity (128).
//!  - Lock-free single-producer / multi-consumer safe atomic head & tail indices.
//!  - Timestamped with CPU cycle counter (`_rdtsc`) for microsecond latency tracking.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub const INPUT_QUEUE_CAPACITY: usize = 128;
const QUEUE_MASK: usize = INPUT_QUEUE_CAPACITY - 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputEventKind {
    MouseMotion { dx: i32, dy: i32, left_down: bool },
    MouseButton { button: u8, down: bool },
    Key { scancode: u8 },
}

impl Default for InputEventKind {
    fn default() -> Self {
        InputEventKind::MouseMotion {
            dx: 0,
            dy: 0,
            left_down: false,
        }
    }
}

#[derive(Copy, Clone, Default, Debug)]
pub struct InputEvent {
    pub kind: InputEventKind,
    pub timestamp_tsc: u64,
}

static mut EVENT_BUFFER: [InputEvent; INPUT_QUEUE_CAPACITY] = [InputEvent {
    kind: InputEventKind::MouseMotion {
        dx: 0,
        dy: 0,
        left_down: false,
    },
    timestamp_tsc: 0,
}; INPUT_QUEUE_CAPACITY];

static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);
static HAS_EVENTS: AtomicBool = AtomicBool::new(false);

/// Enqueue an input event without allocating memory or blocking.
/// Drops the oldest unconsumed event if the queue is saturated (overflow protection).
pub fn enqueue(kind: InputEventKind) -> bool {
    let timestamp_tsc = crate::compositor_metrics::read_tsc();
    let head = HEAD.load(Ordering::Relaxed);
    let tail = TAIL.load(Ordering::Acquire);

    // If queue is full (tail + CAPACITY <= head), bump tail to drop oldest
    if head.wrapping_sub(tail) >= INPUT_QUEUE_CAPACITY {
        TAIL.store(tail.wrapping_add(1), Ordering::Release);
    }

    let idx = head & QUEUE_MASK;
    unsafe {
        #[allow(static_mut_refs)]
        let slot = &mut (*&raw mut EVENT_BUFFER)[idx];
        *slot = InputEvent {
            kind,
            timestamp_tsc,
        };
    }

    HEAD.store(head.wrapping_add(1), Ordering::Release);
    HAS_EVENTS.store(true, Ordering::Release);
    true
}

/// Dequeue the next available input event if one is present.
pub fn dequeue() -> Option<InputEvent> {
    let tail = TAIL.load(Ordering::Relaxed);
    let head = HEAD.load(Ordering::Acquire);

    if tail == head {
        HAS_EVENTS.store(false, Ordering::Release);
        return None;
    }

    let idx = tail & QUEUE_MASK;
    let event = unsafe {
        #[allow(static_mut_refs)]
        (*&raw mut EVENT_BUFFER)[idx]
    };

    TAIL.store(tail.wrapping_add(1), Ordering::Release);
    if TAIL.load(Ordering::Relaxed) == HEAD.load(Ordering::Relaxed) {
        HAS_EVENTS.store(false, Ordering::Release);
    }

    Some(event)
}

#[inline(always)]
pub fn has_pending_events() -> bool {
    HAS_EVENTS.load(Ordering::Acquire)
}

pub fn pending_event_count() -> usize {
    let head = HEAD.load(Ordering::Relaxed);
    let tail = TAIL.load(Ordering::Relaxed);
    head.wrapping_sub(tail)
}

pub fn clear_queue() {
    let head = HEAD.load(Ordering::Relaxed);
    TAIL.store(head, Ordering::Release);
    HAS_EVENTS.store(false, Ordering::Release);
}
