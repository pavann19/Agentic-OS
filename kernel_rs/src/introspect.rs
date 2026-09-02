//! Phase 5 — structured system introspection API (`docs/ROADMAP.md`
//! §5 Phase 5, deliverable 2). "Agent-native rather than agent-on-top":
//! an agent process enumerates the system through typed, machine-legible
//! objects reached via a real syscall, never by scraping this kernel's
//! own `klog_info!` serial output (a human-debugging surface, not an
//! API) or shelling out to anything.
//!
//! `ThreadInfo` is `#[repr(C)]`, fixed-layout, and built directly from
//! `thread::snapshot()`'s real scheduler data — not a re-parsed rendering
//! of a log line. `syscall.rs`'s dispatch for syscall 7 is what actually
//! copies these into the calling process's own buffer, after validating
//! (`vmm::validate_user_buffer_writable`) that the buffer is really
//! theirs to write into, and only after `AGENT_TABLE::resolve` confirms
//! the caller holds a real `Rights::INTROSPECT` capability — enforced by
//! the kernel's own capability check (`docs/ROADMAP.md`'s Phase 5 exit
//! criterion: "a policy denial is enforced by the kernel's capability
//! check, not by the agent's cooperation"), not by the agent choosing to
//! behave.

#![allow(dead_code)]

/// One thread's structured state, exactly as wide as `syscall.rs`'s user-
/// buffer writer expects (`THREAD_INFO_SIZE` below) — kept explicit rather
/// than `core::mem::size_of` at the call site so a future field addition
/// can't silently desync the two without a compile-time mismatch showing
/// up in `write_thread_info`'s own assertion.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ThreadInfo {
    pub id: u64,
    /// 0 = Ready, 1 = Running, 2 = Exited — matches `thread::ThreadState`'s
    /// declaration order; a text label would need parsing, an integer
    /// discriminant doesn't.
    pub state: u32,
    /// 1 = runs in its own process address space (a real "process" in
    /// this kernel's model), 0 = a kernel thread sharing the kernel's own
    /// PML4. u32, not bool, to keep the struct's C layout unambiguous
    /// across the syscall boundary.
    pub is_user: u32,
}

pub const THREAD_INFO_SIZE: u64 = 16; // 8 (id) + 4 (state) + 4 (is_user), no padding at this layout

fn state_discriminant(state: crate::thread::ThreadState) -> u32 {
    match state {
        crate::thread::ThreadState::Ready => 0,
        crate::thread::ThreadState::Running => 1,
        crate::thread::ThreadState::Exited => 2,
    }
}

/// Real typed snapshot, capped to `max_entries` — the cap exists so a
/// caller with a small buffer gets a real, bounded prefix rather than the
/// kernel silently discarding the request or over-running anything; the
/// syscall layer derives `max_entries` from the caller's OWN declared
/// buffer capacity, never a fixed kernel-side guess.
pub fn snapshot_threads(max_entries: usize) -> alloc::vec::Vec<ThreadInfo> {
    crate::thread::snapshot()
        .into_iter()
        .take(max_entries)
        .map(|(id, state, is_user)| ThreadInfo {
            id,
            state: state_discriminant(state),
            is_user: if is_user { 1 } else { 0 },
        })
        .collect()
}

/// Serializes one `ThreadInfo` to its real C-layout bytes, little-endian
/// (this kernel is x86_64-only, so this is also just "native"; explicit
/// or not, the layout has to be someone's contract with user space, so it
/// is stated here rather than left implicit).
/// One capability-scoped audit entry — `syscall.rs` syscall 9's real
/// payload. `kind` mirrors `audit::AuditEvent`'s variant order (0=Grant,
/// 1=Derive, 2=Revoke, 3=Denied, 4=IpcSend, 5=IpcReceive,
/// 6=InterruptDelivered, 7=InterruptAcknowledged, 8=PolicyDenied); `a`/
/// `b` carry that variant's own fields (object_id/rights, cap_id, or
/// vector — zero where a variant doesn't use one), the same "typed
/// fields over one generic pair, not a separate struct per variant"
/// trade-off `ThreadInfo` already makes for a small, fixed, C-stable
/// layout across the syscall boundary.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AuditEntryInfo {
    pub seq: u64,
    pub kind: u32,
    pub a: u32,
    pub b: u32,
    _pad: u32,
}

pub const AUDIT_ENTRY_SIZE: u64 = 24; // 8 + 4*4, 8-byte-aligned

fn event_discriminant(event: crate::audit::AuditEvent) -> (u32, u32, u32) {
    use crate::audit::AuditEvent::*;
    match event {
        Grant { object_id, rights } => (0, object_id, rights),
        Derive { object_id, rights } => (1, object_id, rights),
        Revoke { object_id } => (2, object_id, 0),
        Denied { cap_id } => (3, cap_id, 0),
        IpcSend { object_id } => (4, object_id, 0),
        IpcReceive { object_id } => (5, object_id, 0),
        InterruptDelivered { vector } => (6, vector as u32, 0),
        InterruptAcknowledged { vector } => (7, vector as u32, 0),
        PolicyDenied { rights } => (8, rights, 0),
    }
}

/// Real, capability-scoped snapshot: every audit record the CALLING
/// process's own actions caused (`audit::records_by_actor`), capped to
/// `max_entries`. See that function's own doc for exactly what "the
/// calling process's own actions" means and its one honest limitation
/// (grant attribution goes to the granter, not the grantee).
pub fn snapshot_audit_for(tid: u64, max_entries: usize) -> alloc::vec::Vec<AuditEntryInfo> {
    crate::audit::records_by_actor(tid, max_entries)
        .into_iter()
        .map(|r| {
            let (kind, a, b) = event_discriminant(r.event);
            AuditEntryInfo { seq: r.seq, kind, a, b, _pad: 0 }
        })
        .collect()
}

pub fn audit_entry_bytes(e: &AuditEntryInfo) -> [u8; AUDIT_ENTRY_SIZE as usize] {
    let seq = e.seq.to_le_bytes();
    let kind = e.kind.to_le_bytes();
    let a = e.a.to_le_bytes();
    let b = e.b.to_le_bytes();
    let mut out = [0u8; AUDIT_ENTRY_SIZE as usize];
    let mut i = 0;
    while i < 8 {
        out[i] = seq[i];
        i += 1;
    }
    while i < 12 {
        out[i] = kind[i - 8];
        i += 1;
    }
    while i < 16 {
        out[i] = a[i - 12];
        i += 1;
    }
    while i < 20 {
        out[i] = b[i - 16];
        i += 1;
    }
    // bytes 20..24 stay zero (`_pad`)
    out
}

pub fn thread_info_bytes(info: &ThreadInfo) -> [u8; THREAD_INFO_SIZE as usize] {
    // Field-by-field byte assignment, not `copy_from_slice` — this
    // project's own hard-won lesson (see `kernel_common::mem_intrinsics`'s
    // module doc and Phase 4's PROGRESS.md entry): even small fixed-size
    // slice copies are a pattern LLVM CAN lower into a `memcpy`/`memset`
    // call on this toolchain, and that call resolves through a
    // permanently-unpopulated indirect slot. Explicit per-byte assignment
    // has no such call site to begin with.
    let id = info.id.to_le_bytes();
    let state = info.state.to_le_bytes();
    let is_user = info.is_user.to_le_bytes();
    let mut out = [0u8; THREAD_INFO_SIZE as usize];
    let mut i = 0;
    while i < 8 {
        out[i] = id[i];
        i += 1;
    }
    while i < 12 {
        out[i] = state[i - 8];
        i += 1;
    }
    while i < 16 {
        out[i] = is_user[i - 12];
        i += 1;
    }
    out
}
