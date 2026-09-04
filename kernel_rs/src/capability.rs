//! Capability substrate. Phase 2 — the decision point where this stops
//! being a conventional kernel (`docs/ROADMAP.md`'s own framing). Per
//! ADR-003: no ambient authority anywhere past this point. A process can
//! name a resource ONLY by holding a capability to it; there is no path
//! that lets a caller succeed by virtue of who they are rather than what
//! they hold.
//!
//! Design: two tables, not one.
//!   - A global `OBJECTS` table of `KernelObject`s (what actually exists —
//!     IPC endpoints so far, more kinds as later phases need them). Each
//!     object carries a `generation: u64`.
//!   - A per-process `CapabilityTable`, holding `Capability { object_id,
//!     rights, generation }` entries. `generation` is a COPY of the
//!     object's generation at the moment this specific capability was
//!     minted (granted or derived).
//!
//! Revocation is a generation bump on the OBJECT, not a walk over every
//! outstanding capability that might reference it. `resolve()` — the one
//! path every operation goes through to actually use a capability —
//! rejects any capability whose stored generation does't match the
//! object's CURRENT generation. This is what makes "revocation takes
//! effect immediately, including for already-delegated derivatives" true
//! by construction: a derived capability shares the same object_id, so it
//! shares the same generation counter, so revoking the object invalidates
//! every capability ever minted against it in one write — no tree to walk,
//! no derivative that could be missed.
//!
//! Attenuation is enforced, not advisory: `derive()` computes
//! `new_rights = source.rights & requested_rights` and REJECTS the call
//! outright if `requested_rights` was not already a subset of what the
//! source held — a caller cannot silently receive a weaker capability
//! than asked for and mistake that for success; asking for more than you
//! hold is a real error, not a request that gets quietly clamped.

use alloc::vec::Vec;
use crate::audit;

pub type ObjectId = u32;
pub type CapId = u32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rights(pub u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const SEND: Rights = Rights(1 << 0);
    pub const RECEIVE: Rights = Rights(1 << 1);
    pub const GRANT: Rights = Rights(1 << 2); // may derive/delegate this capability further
    pub const REVOKE: Rights = Rights(1 << 3); // may revoke the underlying object
    // Phase 3: hardware-resource rights. "Nothing ambient" (docs/ROADMAP.md)
    // means these are the ONLY way code reaches MMIO/interrupts/ports —
    // there is no raw-pointer or bare-vector path left once a resource is
    // wrapped as one of these kinds; see driver.rs.
    pub const MAP: Rights = Rights(1 << 4); // may map an MmioRegion into its own address space
    pub const WAIT: Rights = Rights(1 << 5); // may wait_for_interrupt/acknowledge an InterruptLine
    pub const PORT_IO: Rights = Rights(1 << 6); // may issue in/out on a PortIoRange
    // Phase 5: may invoke the structured introspection syscall
    // (`introspect.rs`/`syscall.rs` syscall 7) — enumerate real,
    // typed system state. Its own dedicated right, not reuse of an
    // existing one, so a process holding e.g. `WAIT` on an interrupt
    // never incidentally gains introspection just because some other
    // capability happened to share a bit.
    pub const INTROSPECT: Rights = Rights(1 << 7);
    // Phase 5: may invoke the capability-scoped audit query syscall
    // (`audit.rs::records_by_actor`, syscall 9) -- its own dedicated
    // right, same reasoning as INTROSPECT above.
    pub const AUDIT_QUERY: Rights = Rights(1 << 8);

    pub fn contains(self, other: Rights) -> bool {
        (self.0 & other.0) == other.0
    }
    pub fn intersect(self, other: Rights) -> Rights {
        Rights(self.0 & other.0)
    }
    pub fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }
}

#[derive(Clone, Copy)]
pub enum KernelObjectKind {
    /// An IPC endpoint — Phase 2's one concrete object kind so far. `ipc.rs`
    /// owns the actual mailbox/rendezvous state; this variant just marks
    /// that the object IS an endpoint, giving `resolve_endpoint` something
    /// to type-check against.
    IpcEndpoint,
    /// A physical MMIO range a driver process may map into its OWN address
    /// space (never the kernel's) — `driver.rs::map_mmio`. The capability
    /// carries which physical range; the process never sees or chooses a
    /// physical address itself, only "give me what this capability names."
    MmioRegion { phys_base: u64, size: u64 },
    /// One interrupt vector a driver process may wait for and acknowledge
    /// — `interrupt_forward.rs`, now capability-gated rather than any
    /// caller naming a bare vector number.
    InterruptLine { vector: u8 },
    /// A range of I/O ports a driver process may `in`/`out` on — granted
    /// via the TSS I/O permission bitmap (`driver.rs::grant_port_access`),
    /// not full IOPL=3 (which would open ALL ports, defeating the point of
    /// a capability grant).
    PortIoRange { base: u16, count: u16 },
    /// Phase 4's object store: one file, named by its real ext2 inode
    /// number, not a human-readable path — `object_store.rs`. There is
    /// no separate "list files" or "open by name" API anywhere in this
    /// kernel; a capability IS the only way to name a file at all. A
    /// process without one cannot enumerate, guess into, or otherwise
    /// discover that a given inode's file exists — `resolve()` returns
    /// the exact same `NoSuchCapability` for "this table never held it"
    /// as it would for a bare made-up index, by construction, not by a
    /// separate access-control check layered on top.
    FileObject { inode: u32 },
    /// Phase 5's introspection handle: names no physical resource at all
    /// (unlike every kind above it) — holding it is purely the
    /// permission to call the structured introspection syscall. Carries
    /// no data of its own; `resolve()`'s `Rights::INTROSPECT` check is
    /// the entire access-control surface for it.
    IntrospectionHandle,
    /// Phase 5's audit query handle: same shape as `IntrospectionHandle`
    /// above -- names no resource, exists purely so `Rights::AUDIT_QUERY`
    /// has an object to be granted against, kept separate from
    /// `IntrospectionHandle` so a process holding one right never
    /// incidentally implies the other.
    AuditQueryHandle,
    /// Phase 10 (`docs/ROADMAP.md` §5 deliverable 1 — "a process holds
    /// a socket capability the way it holds any other object, never an
    /// ambient 'the network is just there' model"): one real network
    /// connection, named by protocol and the real local/remote
    /// endpoint it was opened against — not a bare file-descriptor-
    /// style integer a process could guess or iterate. `local_port` is
    /// this endpoint's own real ephemeral port; `remote_ip`/`remote_port`
    /// name who it talks to (0.0.0.0:0 for a not-yet-connected/listening
    /// socket). Revoking this capability (`capability::revoke`, already
    /// real and proven since Phase 2) must make the connection
    /// unusable to the holder immediately -- Phase 10's own third exit
    /// criterion, demonstrated the same adversarial way Phase 2's own
    /// revocation was.
    ///
    /// Real, disclosed scope for this increment: this variant exists
    /// and is real, typed, capability-gated state — the mechanism
    /// `netstack_driver`'s own TCP/UDP implementation will bind actual
    /// connections to is real follow-up work, not yet wired (see
    /// `docs/PROGRESS.md`'s Phase 10 section for exactly what's done vs
    /// open).
    Socket { protocol: SocketProtocol, local_port: u16, remote_ip: [u8; 4], remote_port: u16 },
}

/// Real, small, closed set — matches this stack's own real, from-spec
/// implementations (`netstack_driver`), not a speculative superset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SocketProtocol {
    Udp = 0,
    Tcp = 1,
}

pub struct KernelObject {
    pub kind: KernelObjectKind,
    pub generation: u64,
    pub alive: bool,
}

#[derive(Clone, Copy)]
pub struct Capability {
    pub object_id: ObjectId,
    pub rights: Rights,
    pub generation: u64,
}

static mut OBJECTS: Option<Vec<KernelObject>> = None;
#[allow(static_mut_refs)]
unsafe fn objects_mut() -> &'static mut Vec<KernelObject> {
    if OBJECTS.is_none() {
        OBJECTS = Some(Vec::new());
    }
    (&mut *&raw mut OBJECTS).as_mut().unwrap()
}

/// Creates a new kernel object, returning its id. Not itself capability-
/// gated (there is nothing to gate against yet — this IS the act of
/// bringing the object into existence); the CAPABILITY handed back to the
/// caller by whoever calls this is what's gated from then on.
pub fn create_object(kind: KernelObjectKind) -> ObjectId {
    // Real gap found and closed in the same pass that fixed pmm.rs/
    // heap.rs/thread.rs (see critical.rs's doc comment): this check-
    // then-push on the GLOBAL `OBJECTS` Vec ran with interrupts enabled,
    // and is called from ordinary preemptible thread context by every
    // driver's own setup code (user_driver.rs spawns several such
    // threads) -- a preemption mid-push here is the exact same hazard
    // class already fixed elsewhere, just not caught in the first pass.
    crate::critical::without_interrupts(|| unsafe {
        let objects = objects_mut();
        let id = objects.len() as ObjectId;
        objects.push(KernelObject {
            kind,
            generation: 1,
            alive: true,
        });
        id
    })
}

/// Real, typed count of every object ever created (alive or not) —
/// `smp_race_soak.rs`'s own cross-core race evidence needs a ground
/// truth for "how many objects genuinely exist" that itself goes
/// through the same real, now-cross-core-safe lock every other
/// `OBJECTS` accessor does, not a separately-tracked counter that
/// could drift from the Vec's own real length.
pub fn object_count() -> usize {
    crate::critical::without_interrupts(|| unsafe { objects_mut().len() })
}

/// Returns the kind (and any data it carries — physical range, vector,
/// port range) of a live object, for callers that already resolved a
/// capability against it and now need to know WHAT it actually names.
/// Deliberately takes a raw `ObjectId`, not a capability — this is called
/// AFTER `CapabilityTable::resolve` already did the real access check;
/// this function only answers "what is this", not "may you see it".
pub fn object_kind(object_id: ObjectId) -> Option<KernelObjectKind> {
    crate::critical::without_interrupts(|| unsafe {
        objects_mut()
            .get(object_id as usize)
            .filter(|o| o.alive)
            .map(|o| o.kind)
    })
}

/// Bumps the object's generation (and marks it dead if `destroy` is set).
/// Every capability minted against this object before this call — direct
/// grants AND every derivative — stops resolving as of this line, the
/// instant it runs, no matter how many hops of derivation separate them
/// from the original grant.
pub fn revoke(object_id: ObjectId, destroy: bool) {
    crate::critical::without_interrupts(|| unsafe {
        if let Some(obj) = objects_mut().get_mut(object_id as usize) {
            obj.generation += 1;
            if destroy {
                obj.alive = false;
            }
        }
    });
    audit::record(audit::AuditEvent::Revoke { object_id });
}

/// A per-process capability table: fixed-slot, not a bare array of
/// Options exposed directly — `CapId` is an opaque index into it, never a
/// pointer or a raw object_id a process could forge by guessing.
pub struct CapabilityTable {
    slots: Vec<Option<Capability>>,
}

impl CapabilityTable {
    pub fn new() -> Self {
        CapabilityTable { slots: Vec::new() }
    }

    /// Mints a fresh capability directly from a newly-created object at
    /// full rights — the ONLY way a capability enters a table without
    /// being derived from one already held. Everything after object
    /// creation goes through `derive`.
    pub fn grant(&mut self, object_id: ObjectId, rights: Rights) -> CapId {
        crate::critical::without_interrupts(|| unsafe {
            let generation = objects_mut()
                .get(object_id as usize)
                .map(|o| o.generation)
                .unwrap_or(0);
            let cap = Capability {
                object_id,
                rights,
                generation,
            };
            let id = self.slots.len() as CapId;
            self.slots.push(Some(cap));
            audit::record(audit::AuditEvent::Grant {
                object_id,
                rights: rights.0,
            });
            id
        })
    }

    /// Looks up `cap_id`, checks it against the object's CURRENT
    /// generation (revocation check), and returns the resolved capability
    /// if it's still valid. This is the ONE path every capability-gated
    /// operation in this kernel goes through — invocation and the
    /// generation check are inseparable by construction, matching ADR-005's
    /// "invocation and audit-record emission are one code path" for the
    /// revocation-check half of that guarantee (audit emission itself
    /// happens in the specific operation that calls this, e.g. ipc.rs).
    ///
    /// Real bug found and fixed here (Phase 5's adversarial agent demo,
    /// `agent.rs`, is what surfaced it): the ORIGINAL version of this
    /// function used `?` to bail out immediately on a missing/never-
    /// granted slot (`CapError::NoSuchCapability`), before ever reaching
    /// the `audit::record` call this doc comment claims is unconditional
    /// ("not left to each caller to remember ... regardless of what the
    /// caller does next"). That claim was actually FALSE for exactly the
    /// denial that matters most — a capability-less caller being refused
    /// — while genuinely true for `Revoked`/`InsufficientRights` (the two
    /// error variants reachable past that early return). Concretely: the
    /// stranger process in `agent.rs`'s adversarial demo (and, on closer
    /// inspection, Phase 4's own `OBJSTORE_DISCOVERY_DENIED_OK` stranger-
    /// table case before it) was denied correctly but left ZERO audit
    /// trail of that denial — silently contradicting Phase 5's own exit
    /// criterion ("every agent action ... reconstructible from the audit
    /// log alone"). Fixed by routing the missing-slot case through the
    /// SAME single `result`/audit-on-`Err` path every other failure
    /// already used, rather than a separate early return.
    pub fn resolve(&self, cap_id: CapId, required: Rights) -> Result<Capability, CapError> {
        let slot = self.slots.get(cap_id as usize).and_then(|s| *s);
        let cap = match slot {
            Some(c) => c,
            None => {
                audit::record(audit::AuditEvent::Denied { cap_id });
                return Err(CapError::NoSuchCapability);
            }
        };

        let current_gen = crate::critical::without_interrupts(|| unsafe {
            objects_mut()
                .get(cap.object_id as usize)
                .filter(|o| o.alive)
                .map(|o| o.generation)
        });

        let result = match current_gen {
            Some(g) if g == cap.generation => {
                if cap.rights.contains(required) {
                    Ok(cap)
                } else {
                    Err(CapError::InsufficientRights)
                }
            }
            _ => Err(CapError::Revoked),
        };
        // Denial is logged HERE, inside the one path every capability use
        // goes through — not left to each caller to remember. A caller
        // that forgets to check the Result still can't produce a silent,
        // unaudited denial; the record exists the instant resolution
        // fails, regardless of what the caller does next.
        if result.is_err() {
            audit::record(audit::AuditEvent::Denied { cap_id });
        }
        result
    }

    /// Same as `derive`, but for deriving a new capability into the SAME
    /// table the source lives in ("shrink what I hold myself"). `derive`
    /// takes `&self` and a separate `&mut target` because the normal case
    /// is real delegation into ANOTHER process's table — self-derivation
    /// needs a raw-pointer alias to get past the borrow checker, which is
    /// genuinely safe here: `derive`'s body resolves and copies the
    /// source (an owned `Capability`, by value) BEFORE it ever touches
    /// `target.slots`, so there's no real overlapping access even when
    /// self and target are the same allocation, just a proof the
    /// borrow checker can't do statically.
    pub fn derive_self(&mut self, source_cap_id: CapId, requested_rights: Rights) -> Result<CapId, CapError> {
        let self_ptr = self as *mut CapabilityTable;
        unsafe { (*self_ptr).derive(source_cap_id, requested_rights, &mut *self_ptr) }
    }

    /// Derives a new, attenuated capability from `source_cap_id` and
    /// inserts it into `target` (which may be `self`, for "shrink my own
    /// capability," or another process's table, for real delegation).
    /// `requested_rights` MUST already be a subset of the source's rights
    /// — this is REJECTED outright, not silently clamped, if it isn't;
    /// silently downgrading a request into a "success" would let a caller
    /// believe it received more than it did.
    pub fn derive(
        &self,
        source_cap_id: CapId,
        requested_rights: Rights,
        target: &mut CapabilityTable,
    ) -> Result<CapId, CapError> {
        let source = self.resolve(source_cap_id, Rights::GRANT)?;
        if !source.rights.contains(requested_rights) {
            return Err(CapError::AttenuationViolation);
        }
        Ok(crate::critical::without_interrupts(|| {
            let new_cap = Capability {
                object_id: source.object_id,
                rights: requested_rights,
                generation: source.generation,
            };
            let id = target.slots.len() as CapId;
            target.slots.push(Some(new_cap));
            audit::record(audit::AuditEvent::Derive {
                object_id: source.object_id,
                rights: requested_rights.0,
            });
            id
        }))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CapError {
    NoSuchCapability,
    InsufficientRights,
    Revoked,
    AttenuationViolation,
}
