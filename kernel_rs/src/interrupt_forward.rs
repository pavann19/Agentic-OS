//! Interrupt forwarding to a registered handler. Phase 2 item: "the
//! mechanism Phase 3 drivers depend on" (`docs/ROADMAP.md`) — Phase 3 is
//! where real user-space drivers consume this for real devices; this
//! phase builds and proves the mechanism itself using the one real
//! hardware interrupt source this kernel already has (the APIC timer),
//! since there's no other live IRQ to forward yet (keyboard/PIC stays
//! masked — Phase 0's `pic.rs`).
//!
//! `notify()` is called from the ACTUAL ISR (`idt.rs::h_timer`) — real
//! interrupt context, not simulated. `wait_for_interrupt()` is the
//! consumer side: spin-yields (same documented rendezvous simplification
//! as `ipc.rs`) until a notification is pending, consumes it, and returns.
//! `acknowledge()` is a distinct, separately-audited step — receiving and
//! acknowledging are two different actions with two different audit
//! records, matching the exit criterion's own wording ("receives AND
//! acknowledges").

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use crate::audit;

const MAX_VECTORS: usize = 256;

static REGISTERED: [AtomicBool; MAX_VECTORS] = {
    const F: AtomicBool = AtomicBool::new(false);
    [F; MAX_VECTORS]
};
static PENDING: [AtomicBool; MAX_VECTORS] = {
    const F: AtomicBool = AtomicBool::new(false);
    [F; MAX_VECTORS]
};
static LAST_DELIVERED_VECTOR: AtomicU8 = AtomicU8::new(0);

/// Marks `vector` as forwarded — from this point, `notify()` calls for it
/// mark a pending flag a waiter can consume, rather than (or in addition
/// to) whatever the vector's normal kernel-side ISR already does.
pub fn register(vector: u8) {
    REGISTERED[vector as usize].store(true, Ordering::SeqCst);
}

/// Called from real interrupt context. Safe to call unconditionally on
/// every fire of a vector that MIGHT be registered — a no-op if it isn't.
pub fn notify(vector: u8) {
    if REGISTERED[vector as usize].load(Ordering::SeqCst) {
        PENDING[vector as usize].store(true, Ordering::SeqCst);
    }
}

/// Blocks (spin-yields) until `vector` has a pending notification, then
/// consumes it. This IS "receiving" the interrupt from user space's
/// perspective — logged distinctly from acknowledge() below.
pub fn wait_for_interrupt(vector: u8) {
    while !PENDING[vector as usize].swap(false, Ordering::SeqCst) {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
    LAST_DELIVERED_VECTOR.store(vector, Ordering::SeqCst);
    audit::record(audit::AuditEvent::InterruptDelivered { vector });
}

/// A distinct step from receiving: the consumer explicitly confirms it
/// handled the interrupt. Separated because a real driver's "I saw the
/// IRQ" and "I finished handling it" are genuinely different moments —
/// collapsing them would lose exactly the information an audit trail
/// exists to keep.
pub fn acknowledge(vector: u8) {
    audit::record(audit::AuditEvent::InterruptAcknowledged { vector });
}
