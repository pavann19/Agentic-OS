//! Kernel audit log. Phase 2 item, per ADR-005 (`docs/ROADMAP.md`): "the
//! audit log is a kernel primitive, not a service" — if it were a
//! user-space service processes voluntarily called, an agent that
//! misbehaves is exactly the process least likely to call it. This log is
//! append-only, lives in kernel memory no process has write access to, and
//! `capability.rs`'s `grant`/`derive`/`revoke` call `record()` as part of
//! their own bodies — there is no capability operation whose code path
//! skips this file.
//!
//! Phase 2 scope: an in-memory ring, not yet persisted to disk (there is
//! no filesystem — that's Phase 4). "Invocation and audit-record emission
//! are one code path" is the guarantee THIS phase delivers; durability
//! across a reboot is explicitly later scope, not silently assumed here.

use alloc::collections::VecDeque;
use crate::klog_info;

const MAX_RECORDS: usize = 4096;

#[derive(Clone, Copy, Debug)]
pub enum AuditEvent {
    Grant { object_id: u32, rights: u32 },
    Derive { object_id: u32, rights: u32 },
    Revoke { object_id: u32 },
    Denied { cap_id: u32 },
    IpcSend { object_id: u32 },
    IpcReceive { object_id: u32 },
    InterruptDelivered { vector: u8 },
    InterruptAcknowledged { vector: u8 },
    /// Phase 5's policy engine (`policy.rs`) refused to grant `rights`
    /// at all -- distinct from `Denied`, which is a resolve-time (USE)
    /// refusal of an already-existing capability. This is the earlier,
    /// grant-time refusal: the capability was never minted in the first
    /// place.
    PolicyDenied { rights: u32 },
    /// Phase 6's IOMMU containment exit criterion: a real VT-d DMA
    /// remapping fault was observed (via the Fault Recording Register,
    /// `iommu.rs::poll_and_log_fault`) — a device attempted to DMA
    /// somewhere outside its assigned domain and hardware, not this
    /// kernel's own policy, blocked it. `source_id` is the real
    /// bus/device/function that triggered it (VT-d's own SID field,
    /// bus in the high byte); `reason` is VT-d's own fault-reason code.
    IommuFault { source_id: u16, reason: u8 },
    /// Phase 9.5a: a real user-space driver process died -- `bdf` is
    /// `kernel_common::supervision::pack_bdf`'s packed bus/device/
    /// function, `fault_vector` the real CPU exception vector that
    /// killed it (see `idt.rs::recover_or_halt`, now threading the
    /// real vector through instead of discarding it once the fault was
    /// logged). Distinct from `IommuFault` above -- this is a CPU
    /// exception (idt.rs), not a DMA-containment violation (iommu.rs);
    /// a real driver can die either way, and this event only covers
    /// the former.
    ProcessCrashed { bdf: u32, fault_vector: u8 },
    /// Phase 9.5a: the supervisor (`supervisor.rs`) decided to restart
    /// the crashed device's driver process -- `attempt` is the 1-based
    /// restart attempt number (`kernel_common::supervision::
    /// RestartDecision::Restart`'s own field).
    ProcessRestarted { bdf: u32, attempt: u32 },
    /// Phase 9.5a: the device exhausted `device_manager::MAX_RESTARTS`
    /// and was left permanently `Failed` rather than restarted again --
    /// real evidence a bounded retry budget was honored, not an
    /// unbounded restart loop.
    ProcessQuarantined { bdf: u32 },
    /// Phase 13 deliverable 1 (`docs/ROADMAP.md` §5): an app's install
    /// manifest did not declare the capability KIND a grant at spawn
    /// time would have minted -- refused before the object ever reached
    /// the new process's table, same "grant-time, not use-time" shape
    /// as `PolicyDenied` above, but keyed on a per-app manifest
    /// (`manifest.rs`) rather than the one global agent policy
    /// (`policy.rs`). `kind` is `manifest::CapKind`'s own discriminant,
    /// cast to u8 -- coarse on purpose, matching the manifest's own
    /// granularity (declares KINDS of capability, never specific
    /// instances).
    ManifestDenied { kind: u8 },
}

#[derive(Clone, Copy)]
pub struct AuditRecord {
    pub seq: u64,
    /// Phase 5: which thread's OWN context this record was created
    /// under (`thread::current_id()` at the moment of `record()`), NOT
    /// necessarily "the process this event is ABOUT" — a `Grant` issued
    /// by a spawning/orchestrating thread on a new process's behalf
    /// (see `thread::spawn_with_capabilities`) is attributed to the
    /// SPAWNER, since that's who was actually executing when the grant
    /// happened. A `Denied`/`PolicyDenied` from inside a syscall,
    /// though, IS attributed to the actual calling process — syscall
    /// dispatch runs on the calling thread's own context, so
    /// `records_by_actor` genuinely answers "what did MY OWN actions
    /// cause to be audited" for exactly the case that matters most: an
    /// agent's own denied attempts. Stated honestly rather than
    /// papered over — full "who benefits from this capability"
    /// attribution for every event kind is real future work.
    pub actor_tid: u64,
    pub event: AuditEvent,
}

static mut LOG: Option<VecDeque<AuditRecord>> = None;
static mut NEXT_SEQ: u64 = 0;

#[allow(static_mut_refs)]
unsafe fn log_mut() -> &'static mut VecDeque<AuditRecord> {
    if LOG.is_none() {
        LOG = Some(VecDeque::new());
    }
    (&mut *&raw mut LOG).as_mut().unwrap()
}

/// Appends one record. Bounded (`MAX_RECORDS`) — a real system would spill
/// to persistent storage before ever hitting this ring's capacity (Phase
/// 4 concern); dropping the oldest record here rather than growing
/// unboundedly is a deliberate, stated memory-safety choice for a kernel
/// structure with no eviction policy otherwise, not a silent data-loss
/// bug — `MAX_RECORDS` is generous enough that Phase 2's own demos never
/// come close to it.
/// Real gap found and closed alongside pmm.rs/heap.rs/thread.rs/
/// capability.rs (see critical.rs's doc comment): this function is
/// called from ordinary preemptible thread context by MANY unrelated
/// callers (every capability grant/derive/revoke/resolve, every IPC
/// send/receive, interrupt_forward's wait/acknowledge) — a preemption
/// mid-push on the shared `LOG` VecDeque let two callers corrupt it the
/// same way every other unprotected global mutation in this kernel
/// could before this pass.
pub fn record(event: AuditEvent) {
    // Read OUTSIDE the critical section below -- `thread::current_id()`
    // takes its own `critical::without_interrupts` lock internally;
    // nesting is safe (see critical.rs) but reading it first here keeps
    // this function's own lock section to exactly the LOG mutation, not
    // a second independent lookup it doesn't need to cover.
    let actor_tid = crate::thread::current_id();
    crate::critical::without_interrupts(|| unsafe {
        let seq = NEXT_SEQ;
        NEXT_SEQ += 1;
        let log = log_mut();
        if log.len() >= MAX_RECORDS {
            log.pop_front();
        }
        log.push_back(AuditRecord { seq, actor_tid, event });
    });
}

pub fn len() -> usize {
    crate::critical::without_interrupts(|| unsafe { log_mut().len() })
}

pub fn last_n(n: usize) -> alloc::vec::Vec<AuditRecord> {
    crate::critical::without_interrupts(|| unsafe {
        let log = log_mut();
        let skip = log.len().saturating_sub(n);
        log.iter().skip(skip).copied().collect()
    })
}

/// Phase 5's capability-scoped audit query primitive
/// (`docs/ROADMAP.md` §5 Phase 5, deliverable 5): every record whose
/// `actor_tid` matches `tid`, most-recent-first, capped at `max`. This
/// is what makes the syscall surface (`syscall.rs` syscall 9) genuinely
/// "capability-scoped" rather than "the whole log with an extra
/// permission check" — a caller gets exactly its OWN slice, structurally,
/// not the full log filtered client-side (which would still have
/// required trusting the log itself was safe to expose wholesale).
pub fn records_by_actor(tid: u64, max: usize) -> alloc::vec::Vec<AuditRecord> {
    crate::critical::without_interrupts(|| unsafe {
        log_mut()
            .iter()
            .rev()
            .filter(|r| r.actor_tid == tid)
            .take(max)
            .copied()
            .collect()
    })
}

/// Dumps the whole log to serial — a diagnostic/demo tool, not how a real
/// consumer (a Phase 5 agent-facing audit query interface) would read it.
pub fn dump_all() {
    crate::critical::without_interrupts(|| unsafe {
        for r in log_mut().iter() {
            klog_info!("AUDIT seq={} actor_tid={} event={:?}", r.seq, r.actor_tid, r.event);
        }
    });
}
