//! Phase 13 deliverable 1 (`docs/ROADMAP.md` §5): "An application
//! manifest format declaring required capabilities up front —
//! install-time, explicit, least-privilege by construction... consistent
//! with ADR-003 from the very first syscall this OS ever had."
//!
//! Deliberately distinct from `policy.rs`'s existing grant-time engine:
//! `policy.rs` enforces ONE global rule (`AGENT_MAX_RIGHTS`) against
//! every agent process alike. A real app platform needs a PER-APP rule
//! instead — app A's manifest may declare `Surface`, app B's may declare
//! `Socket`, and granting app A a `Socket` capability must be refused
//! even though the exact same grant would be fine for app B. This module
//! is that per-app check, keyed on capability KIND (coarse, matching a
//! manifest's own declared granularity — "this app may hold a Surface",
//! never "this app may hold a Surface at exactly (x, y)"), not on the
//! `Rights` bitmask `policy.rs` uses. The two checks are independent:
//! this module does not replace `policy.rs`, it adds the axis `policy.rs`
//! was never designed to cover. Real enforcement lives in
//! `thread::spawn_with_manifest`, this module's own natural home
//! (`policy.rs`'s equivalent check lives inside `thread::
//! spawn_with_capabilities` for the identical reason: the grant loop
//! that needs to consult it is there, not here).

use crate::capability::KernelObjectKind;

/// Coarse capability-kind discriminant, one per `KernelObjectKind`
/// variant (`capability.rs`) — deliberately NOT the variant's own data
/// (an app manifest declares "may hold a FileObject", never "may hold
/// inode 42 specifically"; which concrete object gets minted is still
/// entirely up to whatever kernel-side code does the minting, exactly
/// as it is for every driver today).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CapKind {
    IpcEndpoint = 0,
    MmioRegion = 1,
    InterruptLine = 2,
    PortIoRange = 3,
    FileObject = 4,
    IntrospectionHandle = 5,
    AuditQueryHandle = 6,
    Socket = 7,
    Surface = 8,
    UserSession = 9,
    CryptoKey = 10,
}

pub(crate) fn kind_of(kind: &KernelObjectKind) -> CapKind {
    use KernelObjectKind::*;
    match kind {
        IpcEndpoint => CapKind::IpcEndpoint,
        MmioRegion { .. } => CapKind::MmioRegion,
        InterruptLine { .. } => CapKind::InterruptLine,
        PortIoRange { .. } => CapKind::PortIoRange,
        FileObject { .. } => CapKind::FileObject,
        IntrospectionHandle => CapKind::IntrospectionHandle,
        AuditQueryHandle => CapKind::AuditQueryHandle,
        Socket { .. } => CapKind::Socket,
        Surface { .. } => CapKind::Surface,
        UserSession { .. } => CapKind::UserSession,
        CryptoKey { .. } => CapKind::CryptoKey,
    }
}

/// A real, per-app allow-list — a bitmask over `CapKind`'s own
/// discriminants, same "reject outright, never silently narrow"
/// discipline `policy.rs`/`CapabilityTable::derive` already use. Built
/// once (today, by whatever kernel-side code stands in for the not-yet-
/// built package/installer service — deliverable 2 of this same phase)
/// and carried alongside the app's grant list into `thread::
/// spawn_with_manifest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Manifest(pub u16);

impl Manifest {
    pub const NONE: Manifest = Manifest(0);

    pub fn allow(self, kind: CapKind) -> Manifest {
        Manifest(self.0 | (1 << kind as u16))
    }

    pub fn declares(self, kind: CapKind) -> bool {
        (self.0 & (1 << kind as u16)) != 0
    }
}
