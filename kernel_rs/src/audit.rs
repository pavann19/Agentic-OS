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
}

#[derive(Clone, Copy)]
pub struct AuditRecord {
    pub seq: u64,
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
    crate::critical::without_interrupts(|| unsafe {
        let seq = NEXT_SEQ;
        NEXT_SEQ += 1;
        let log = log_mut();
        if log.len() >= MAX_RECORDS {
            log.pop_front();
        }
        log.push_back(AuditRecord { seq, event });
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

/// Dumps the whole log to serial — a diagnostic/demo tool, not how a real
/// consumer (a Phase 5 agent-facing audit query interface) would read it.
pub fn dump_all() {
    crate::critical::without_interrupts(|| unsafe {
        for r in log_mut().iter() {
            klog_info!("AUDIT seq={} event={:?}", r.seq, r.event);
        }
    });
}
