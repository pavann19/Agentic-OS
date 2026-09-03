//! Phase 5 — policy engine (`docs/ROADMAP.md` §5 Phase 5, deliverable
//! 4): "declarative rules over which capabilities an agent may hold and
//! exercise, enforced at grant time." Deliberately distinct from
//! `capability.rs::CapabilityTable::resolve`, which enforces at USE
//! time (does this specific already-held capability cover this
//! specific operation) — this module enforces one step earlier, at the
//! moment a capability would be minted into an agent's table at all.
//! The two checks are independent and both real: a right this module
//! refuses to grant can never reach `resolve()` in the first place; a
//! right this module WOULD allow still has to pass `resolve()`'s own
//! generation/rights check on every actual use.
//!
//! Scope, stated honestly: this is ONE global, compile-time-declarative
//! rule (a single allow-list bitmask), not a general per-agent-class
//! policy table with runtime-loaded rules — that's real future work,
//! not pretended to exist here. What IS here is genuinely enforced, not
//! a stub: see `thread::spawn_with_capabilities`, the one place in this
//! kernel a capability is minted directly into a new agent process's
//! table, which calls `allows()` before every single grant and refuses
//! (logging why) any that fails it.

use crate::capability::Rights;

/// The declarative rule: every capability an agent process may ever be
/// granted, unioned into one mask. Currently: introspection and
/// audit-query only — no agent gets a hardware capability (MAP/WAIT/
/// PORT_IO) through this path. Widening this is a real, deliberate
/// policy change a future phase would make explicitly (e.g. once a real
/// hardware-capable agent use case exists) — not something any single
/// grant call can talk its way around.
const AGENT_MAX_RIGHTS: Rights = Rights(Rights::INTROSPECT.0 | Rights::AUDIT_QUERY.0);

/// Real grant-time check: is `rights` entirely covered by this policy?
/// Same "reject outright, never silently clamp" discipline
/// `CapabilityTable::derive` already uses for attenuation — a caller
/// asking for a right this policy disallows gets a clean refusal for
/// THAT grant, not a silently narrowed one it could mistake for having
/// gotten what it asked for.
pub fn allows(rights: Rights) -> bool {
    AGENT_MAX_RIGHTS.contains(rights)
}
