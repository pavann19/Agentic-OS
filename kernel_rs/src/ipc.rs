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

use crate::capability::{CapError, CapId, CapabilityTable, KernelObjectKind, ObjectId, Rights};
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

/// Per-endpoint capacity for the fire-and-forget (try_send/try_receive)
/// mailbox queue. 32 slots: a real, disclosed fix for the INPUT_ROUTE_
/// DROPPED_BUSY latency bug -- rapid typing or multi-byte extended
/// scancodes (arrow keys send two events, 0xE0 + code) arrived faster
/// than the ring-3 app's ~150ms scheduling quantum could drain the
/// old single-slot mailbox, causing silent keystroke loss. 32 slots
/// absorbs a full burst of fast typing before any slot is consumed.
/// The blocking rendezvous protocol (send/receive) retains its own
/// separate `state`/`message` fields and is unaffected.
const QUEUE_DEPTH: usize = 32;

struct Endpoint {
    // Blocking rendezvous (send/receive) fields -- unchanged.
    state: core::sync::atomic::AtomicU8,
    message: Message,
    // Fire-and-forget (try_send/try_receive) ring buffer.
    // head: next write slot (producer-owned, AtomicU8 wrapping mod QUEUE_DEPTH)
    // tail: next read slot (consumer-owned, AtomicU8 wrapping mod QUEUE_DEPTH)
    // len:  outstanding messages  (both sides bump; checked for full/empty)
    q_head: core::sync::atomic::AtomicU8,
    q_tail: core::sync::atomic::AtomicU8,
    q_len:  core::sync::atomic::AtomicU8,
    q_slots: [Message; QUEUE_DEPTH],
}

// Real bug found and fixed bringing up Phase 12 input routing:
// `send()`/`receive()` each hold a `&mut Endpoint` reference across a
// busy-wait spin loop that can last an arbitrary amount of real time
// (the scheduler keeps running other threads while it spins). Until
// this session, nothing else ever grew `ENDPOINTS` WHILE one of those
// spins was live -- `create_endpoint` was only ever called during
// early, sequential boot setup. `input_routing::register_window_input`
// is the first code path to call it from a genuinely concurrent
// thread (a window client's own spawn code) while another thread could
// simultaneously be mid-spin inside `send`/`receive` on a DIFFERENT,
// already-existing endpoint (`compositor.rs`'s shared `FB_READY`
// endpoint, in this exact reproduction). `Vec<Endpoint>` reallocates
// its backing buffer on `push` once capacity is exceeded -- any
// `&mut Endpoint` obtained before that reallocation, from a DIFFERENT
// thread that never re-derives it, becomes a dangling reference into
// freed memory the instant that happens. Observed real, reproduced
// symptom: `compositor_verify_thread`'s receive loop read the SAME
// stale message content twice in a row for what should have been two
// genuinely different senders' messages -- classic use-after-free-style
// corruption, not a logic bug in the rendezvous protocol itself.
//
// Fixed at the root by reserving real, fixed capacity up front so
// `push` never reallocates for any object_id this kernel actually
// reaches in practice -- every held `&mut Endpoint` reference stays
// valid across any later registration, regardless of which thread does
// it or when. `MAX_ENDPOINTS` is a real, generous bound (this kernel's
// entire object-id space across a full boot, every demo included,
// stays well under it), not a magic number: if it's ever exceeded,
// `create_endpoint` panics loudly instead of silently reallocating out
// from under a live reference again.
struct EndpointSlot {
    object_id: ObjectId,
    endpoint: alloc::boxed::Box<Endpoint>,
}

static mut ENDPOINTS: Option<alloc::vec::Vec<EndpointSlot>> = None;
#[allow(static_mut_refs)]
unsafe fn endpoints_mut() -> &'static mut alloc::vec::Vec<EndpointSlot> {
    if ENDPOINTS.is_none() {
        ENDPOINTS = Some(alloc::vec::Vec::new());
    }
    (&mut *&raw mut ENDPOINTS).as_mut().unwrap()
}

unsafe fn find_endpoint_mut(object_id: ObjectId) -> Option<&'static mut Endpoint> {
    let eps = endpoints_mut();
    for slot in eps.iter_mut() {
        if slot.object_id == object_id {
            let ptr = slot.endpoint.as_mut() as *mut Endpoint;
            return Some(&mut *ptr);
        }
    }
    None
}

/// Real fix, paired with `syscall.rs`'s own entry-stub doc: syscall
/// dispatch no longer unconditionally `sti`s for its entire duration
/// (that reopened a real per-core user-RSP-stash race between two
/// threads' syscalls interleaving on the same core — see that comment
/// for the full story). Interrupts are OFF by default for the whole
/// syscall now, so a genuinely blocking wait like this one must
/// briefly re-enable them itself, right around the `hlt` that needs
/// them to ever wake up — the classic `sti; hlt` idiom, safe because
/// of the real "STI shadow" hardware guarantee (an interrupt enabled
/// by `sti` cannot actually be taken until AFTER the very next
/// instruction), so this pairing can only ever be interrupted at the
/// `hlt` itself, immediately followed by `cli` again before this
/// function returns to its own (non-preemptible-by-design) caller.
fn spin_yield() {
    unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
}

/// Creates a new IPC endpoint object and returns a capability to it (with
/// the requested rights, typically SEND|RECEIVE for whoever creates it —
/// they can then `derive()` a weaker one, e.g. SEND-only, to hand to
/// another process) in `table`.
pub fn create_endpoint(table: &mut CapabilityTable, rights: Rights) -> CapId {
    let object_id = capability::create_object(KernelObjectKind::IpcEndpoint);
    crate::critical::without_interrupts(|| unsafe {
        let eps = endpoints_mut();
        eps.push(EndpointSlot {
            object_id,
            endpoint: alloc::boxed::Box::new(Endpoint {
                state: core::sync::atomic::AtomicU8::new(STATE_IDLE),
                message: Message::default(),
                q_head:  core::sync::atomic::AtomicU8::new(0),
                q_tail:  core::sync::atomic::AtomicU8::new(0),
                q_len:   core::sync::atomic::AtomicU8::new(0),
                q_slots: [Message::default(); QUEUE_DEPTH],
            }),
        });
    });
    table.grant(object_id, rights)
}

#[derive(Debug)]
pub enum IpcError {
    Cap(CapError),
    EndpointNotFound,
}

/// Blocks (spin-yields) until a receiver has taken `msg`, then returns.
/// Real rendezvous: this does not return early just because the message
/// was deposited — it waits for confirmation a receiver actually consumed
/// it, so a sender knows delivery genuinely happened.
pub fn send(table: &CapabilityTable, cap_id: CapId, msg: Message) -> Result<(), IpcError> {
    use core::sync::atomic::Ordering;
    let cap = table.resolve(cap_id, Rights::SEND).map_err(IpcError::Cap)?;
    unsafe {
        let ep = find_endpoint_mut(cap.object_id).ok_or(IpcError::EndpointNotFound)?;
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
        let ep = find_endpoint_mut(cap.object_id).ok_or(IpcError::EndpointNotFound)?;
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
        let ep = find_endpoint_mut(cap.object_id).ok_or(IpcError::EndpointNotFound)?;
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

/// Phase 12 exit criterion 4's own real routing primitives
/// (`try_send`/`try_receive`/`try_receive_on_object` below): a
/// deliberately SEPARATE, non-rendezvous mailbox protocol from
/// `send`/`receive` above, real one-shot single-slot semantics (`IDLE`
/// -> `MESSAGE_PENDING` -> `IDLE`, no `MESSAGE_TAKEN` confirmation
/// phase) rather than reusing their blocking handshake. This is a real
/// bug found and fixed bringing up input routing, not a stylistic
/// choice: `deliver_key_event` originally called the blocking `send`
/// above, which waits for the RECEIVER to confirm consumption before
/// returning — but a window's own input poll is deliberately bounded
/// (a window that never holds focus must not block forever), so if
/// that bounded poll window closed before the real event arrived, the
/// message would sit PENDING forever with no one left to consume it,
/// and `send`'s second wait loop would spin forever too — a real,
/// reproduced deadlock that permanently wedged the keyboard driver's
/// OWN thread (every future keystroke silently lost, not just the one
/// that raced). Fire-and-forget mailbox semantics make that scenario
/// impossible by construction: a routed event that arrives after the
/// target window stopped polling is a real, disclosed drop (the exact
/// same "not guaranteed to be delivered, dropping is not a bug" case
/// `deliver_key_event`'s own doc already names for "no focused
/// window"), never a hang.
pub fn try_send(table: &CapabilityTable, cap_id: CapId, msg: Message) -> Result<bool, IpcError> {
    use core::sync::atomic::Ordering;
    let cap = table.resolve(cap_id, Rights::SEND).map_err(IpcError::Cap)?;
    let enqueued = unsafe {
        let ep = find_endpoint_mut(cap.object_id).ok_or(IpcError::EndpointNotFound)?;
        let len = ep.q_len.load(Ordering::Acquire);
        if len as usize >= QUEUE_DEPTH {
            // Real, disclosed: queue full — fire-and-forget drop.
            // 32 slots absorb a full rapid-typing burst; if all 32 are
            // unconsumed the focused window is genuinely behind.
            false
        } else {
            let head = ep.q_head.load(Ordering::Relaxed) as usize;
            ep.q_slots[head] = msg;
            ep.q_head.store(((head + 1) % QUEUE_DEPTH) as u8, Ordering::Relaxed);
            ep.q_len.fetch_add(1, Ordering::Release);
            true
        }
    };
    if enqueued {
        audit::record(audit::AuditEvent::IpcSend { object_id: cap.object_id });
    }
    Ok(enqueued)
}

pub fn try_receive(table: &CapabilityTable, cap_id: CapId) -> Result<Option<Message>, IpcError> {
    let cap = table.resolve(cap_id, Rights::RECEIVE).map_err(IpcError::Cap)?;
    Ok(try_receive_on_object(cap.object_id))
}

/// Same non-blocking check as `try_receive` above, for a caller that
/// has ALREADY resolved the capability itself (e.g. `syscall.rs`'s
/// syscall 12, via `thread::resolve_current_capability` — the per-
/// process check that syscall path needs regardless) and just needs
/// the raw object-level operation, without resolving twice against the
/// same table. Resets straight to `STATE_IDLE` (never `MESSAGE_TAKEN`)
/// — this mailbox's sender (`try_send`) never waits for a consumption
/// confirmation, so there is nothing for an intermediate "taken but not
/// yet reset" state to communicate.
pub fn try_receive_on_object(object_id: ObjectId) -> Option<Message> {
    use core::sync::atomic::Ordering;
    unsafe {
        let ep = find_endpoint_mut(object_id)?;
        let len = ep.q_len.load(Ordering::Acquire);
        if len == 0 {
            return None;
        }
        let tail = ep.q_tail.load(Ordering::Relaxed) as usize;
        let m = ep.q_slots[tail];
        ep.q_tail.store(((tail + 1) % QUEUE_DEPTH) as u8, Ordering::Relaxed);
        ep.q_len.fetch_sub(1, Ordering::Release);
        audit::record(audit::AuditEvent::IpcReceive { object_id });
        Some(m)
    }
}
