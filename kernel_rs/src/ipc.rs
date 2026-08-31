//! Synchronous, capability-gated IPC. Phase 2 item. An endpoint is a
//! `capability::KernelObject` (kind `IpcEndpoint`); this module owns the
//! actual mailbox state, keyed by the same `ObjectId` capability.rs uses,
//! so a capability's `object_id` is directly the index into both tables.
//!
//! Rendezvous simplification, stated plainly (same pattern as
//! `syscall.rs`'s single-core stack pointer): "blocking" here is a spin
//! loop that yields via `hlt` each iteration, relying on the preemptive
//! scheduler (Phase 1) to run other threads while this one waits — not a
//! real blocked/wait-queue thread state (`thread.rs` only has
//! Ready/Running/Exited so far). This is CORRECT (a sender genuinely
//! doesn't proceed until a receiver has taken the message, and vice
//! versa) but wastes CPU busy-polling instead of truly sleeping. Real
//! blocking — moving a waiting thread out of the ready queue entirely —
//! is a stated future improvement once `thread.rs` grows a Blocked state,
//! not a correctness gap in what Phase 2 claims.

use crate::capability::{CapError, CapId, CapabilityTable, KernelObjectKind, Rights};
use crate::{audit, capability};

#[derive(Clone, Copy, Default)]
pub struct Message {
    pub data: [u64; 4],
}

// Plain enum + plain field, accessed through a normal `&mut` reference,
// was a REAL bug found by testing: spin_yield()'s `asm!("hlt", ...)` is
// marked `options(nomem, nostack)`, which tells LLVM the asm touches no
// memory at all -- so the compiler is free to hoist `ep.state`'s read out
// of the wait loop entirely (nothing in the loop body, from its point of
// view, could change it), spinning on a cached, stale register value
// forever. The sender's second wait (for MessageTaken) hung permanently
// this way; the receiver's own state-change was real and visible in
// memory, just never re-read. Fixed with a genuine atomic and
// Acquire/Release ordering -- both correct semantically (this IS
// cross-context shared mutable state, exactly what atomics are for) and
// immune to the hoisting problem, since atomic loads are never assumed
// invariant across an opaque call the way a plain field read is.
const STATE_IDLE: u8 = 0;
const STATE_MESSAGE_PENDING: u8 = 1;
const STATE_MESSAGE_TAKEN: u8 = 2;

struct Endpoint {
    state: core::sync::atomic::AtomicU8,
    message: Message,
}

static mut ENDPOINTS: Option<alloc::vec::Vec<Endpoint>> = None;
#[allow(static_mut_refs)]
unsafe fn endpoints_mut() -> &'static mut alloc::vec::Vec<Endpoint> {
    if ENDPOINTS.is_none() {
        ENDPOINTS = Some(alloc::vec::Vec::new());
    }
    (&mut *&raw mut ENDPOINTS).as_mut().unwrap()
}

fn spin_yield() {
    unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
}

/// Creates a new IPC endpoint object and returns a capability to it (with
/// the requested rights, typically SEND|RECEIVE for whoever creates it —
/// they can then `derive()` a weaker one, e.g. SEND-only, to hand to
/// another process) in `table`.
pub fn create_endpoint(table: &mut CapabilityTable, rights: Rights) -> CapId {
    let object_id = capability::create_object(KernelObjectKind::IpcEndpoint);
    // Same real gap class as capability.rs/audit.rs (see critical.rs) --
    // this Vec growth ran with interrupts enabled, called from ordinary
    // preemptible thread context by multiple driver setup threads.
    crate::critical::without_interrupts(|| unsafe {
        let eps = endpoints_mut();
        while eps.len() <= object_id as usize {
            eps.push(Endpoint {
                state: core::sync::atomic::AtomicU8::new(STATE_IDLE),
                message: Message::default(),
            });
        }
    });
    table.grant(object_id, rights)
}

#[derive(Debug)]
pub enum IpcError {
    Cap(CapError),
}

/// Blocks (spin-yields) until a receiver has taken `msg`, then returns.
/// Real rendezvous: this does not return early just because the message
/// was deposited — it waits for confirmation a receiver actually consumed
/// it, so a sender knows delivery genuinely happened.
pub fn send(table: &CapabilityTable, cap_id: CapId, msg: Message) -> Result<(), IpcError> {
    use core::sync::atomic::Ordering;
    let cap = table.resolve(cap_id, Rights::SEND).map_err(IpcError::Cap)?;
    unsafe {
        let ep = &mut endpoints_mut()[cap.object_id as usize];
        while ep.state.load(Ordering::Acquire) != STATE_IDLE {
            spin_yield();
        }
        // Plain (non-atomic) field write, safe here specifically because
        // the Release store right after it establishes a happens-before
        // edge with the receiver's matching Acquire load — the receiver
        // is guaranteed to see this write once it observes
        // STATE_MESSAGE_PENDING, never before.
        ep.message = msg;
        ep.state.store(STATE_MESSAGE_PENDING, Ordering::Release);
    }
    audit::record(audit::AuditEvent::IpcSend {
        object_id: cap.object_id,
    });
    unsafe {
        let ep = &mut endpoints_mut()[cap.object_id as usize];
        while ep.state.load(Ordering::Acquire) != STATE_MESSAGE_TAKEN {
            spin_yield();
        }
        ep.state.store(STATE_IDLE, Ordering::Release);
    }
    Ok(())
}

/// Blocks (spin-yields) until a message is pending, takes it.
pub fn receive(table: &CapabilityTable, cap_id: CapId) -> Result<Message, IpcError> {
    use core::sync::atomic::Ordering;
    let cap = table.resolve(cap_id, Rights::RECEIVE).map_err(IpcError::Cap)?;
    let msg = unsafe {
        let ep = &mut endpoints_mut()[cap.object_id as usize];
        while ep.state.load(Ordering::Acquire) != STATE_MESSAGE_PENDING {
            spin_yield();
        }
        let m = ep.message; // see send()'s comment: safe post-Acquire
        ep.state.store(STATE_MESSAGE_TAKEN, Ordering::Release);
        m
    };
    audit::record(audit::AuditEvent::IpcReceive {
        object_id: cap.object_id,
    });
    Ok(msg)
}
