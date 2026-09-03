//! Deferred interrupt work queue — `docs/ROADMAP.md` Phase 0 item: "IRQ
//! handlers capture event state into a queue and return... All rendering,
//! parsing, and scheduling moves out of interrupt context." The C
//! kernel's keyboard handler was the named example of what NOT to do (it
//! rendered directly to the framebuffer from inside the ISR); this is the
//! real infrastructure so nothing ported after Phase 0 repeats that
//! mistake, not just an observation that the one handler that exists so
//! far happens to already be minimal.
//!
//! Fixed-capacity SPSC (single-producer single-consumer) ring buffer:
//! interrupt handlers are the one producer, `main.rs`'s main loop is the
//! one consumer. Lock-free (interrupt context must never block on a lock
//! the main loop might be holding — that's a deadlock waiting to happen
//! the moment an interrupt fires mid-critical-section), backed by atomics
//! rather than a spinlock for exactly that reason.

use core::sync::atomic::{AtomicUsize, Ordering};

const CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Tick(u64),
    // Future producers (keyboard, other IRQs) add variants here — that's
    // the point of this existing before any of them do.
}

struct RingBuffer {
    slots: [Option<Event>; CAPACITY],
    head: AtomicUsize, // next slot to write (producer-owned)
    tail: AtomicUsize, // next slot to read (consumer-owned)
}

static mut QUEUE: RingBuffer = RingBuffer {
    slots: [None; CAPACITY],
    head: AtomicUsize::new(0),
    tail: AtomicUsize::new(0),
};

/// Safe to call from interrupt context: no locks, bounded work (a handful
/// of atomic ops), never blocks. Drops the event if the queue is full
/// rather than blocking or growing — an interrupt handler must return
/// promptly regardless of consumer backpressure; a full queue means the
/// consumer isn't draining fast enough, which is a real condition to
/// eventually surface (dropped-event counter — not built yet, a stated
/// gap) rather than something interrupt context can fix by waiting.
pub fn push(event: Event) {
    unsafe {
        // One raw pointer, used consistently, rather than repeated
        // `QUEUE.field` accesses that each implicitly form a shared
        // reference to the mutable static — the AtomicUsize fields are
        // genuinely fine to alias (that's what atomics are for), but the
        // lint doesn't know that, and going through one pointer either way
        // is no more code.
        let q = &raw mut QUEUE;
        let head = (*q).head.load(Ordering::Relaxed);
        let next = (head + 1) % CAPACITY;
        if next == (*q).tail.load(Ordering::Acquire) {
            return; // full — drop
        }
        (*q).slots[head] = Some(event);
        (*q).head.store(next, Ordering::Release);
    }
}

/// Consumer side — call from normal (non-interrupt) kernel context only.
pub fn pop() -> Option<Event> {
    unsafe {
        let q = &raw mut QUEUE;
        let tail = (*q).tail.load(Ordering::Relaxed);
        if tail == (*q).head.load(Ordering::Acquire) {
            return None; // empty
        }
        let event = (*q).slots[tail].take();
        (*q).tail.store((tail + 1) % CAPACITY, Ordering::Release);
        event
    }
}
