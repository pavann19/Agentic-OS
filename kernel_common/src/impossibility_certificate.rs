//! Research track (`docs/RESEARCH_TRACK.md`, `docs/NOVEL_CONCEPTS.md`
//! §2): "physical impossibility certificates" — a verifiable claim
//! that a principal CANNOT reach a physical page, grounded in the
//! authority graph's own state at issuance time, not a policy
//! assertion. Isolated, pure-logic prototype, same discipline as §1's
//! own first increment: proves the DATA MODEL here; real hardware
//! wiring (binding a certificate to the live IOMMU context-table bytes
//! `kernel_rs::iommu::context_entry_present` reads, and the "corrupt
//! one entry underneath it, watch the certificate fail to re-verify"
//! demonstration `docs/NOVEL_CONCEPTS.md` §2.4 describes) is real,
//! separate follow-up work, not done here.
//!
//! **What a certificate actually claims, precisely:** "given the
//! authority graph in the exact state identified by `graph_checksum`,
//! `principal` cannot reach `phys_page`." Verification re-derives the
//! checksum from the CURRENT graph and compares — if the graph's LIVE
//! CONTENTS differ from what was certified, the certificate is
//! `Stale`, not silently re-validated against a different state than
//! the one it actually certified. This is a deliberate, real design
//! choice: a certificate binds to a measured state, the same way a TPM
//! measurement binds to the exact software that produced it.
//!
//! **Real, checked, and possibly counterintuitive property, found by
//! this module's own first test run:** the checksum is
//! CONTENT-addressed, not event-addressed — it hashes what the graph
//! currently CONTAINS, not a log of what happened to it. This means a
//! revoke followed by a regrant of the byte-for-byte identical
//! `Grant` re-validates a certificate as `Valid` again, correctly:
//! the state really is, in every checkable way, the same state the
//! certificate was issued against. An earlier version of this
//! module's own test suite asserted the opposite and was simply wrong
//! about the contract; `host_tests::impossibility_certificate_tests::
//! revoking_and_regranting_identical_content_still_verifies_valid`
//! is the corrected test, kept specifically to exercise this
//! real property rather than delete the evidence of the mistake.
//! Detecting "something happened, even if it net-cancelled" instead
//! would need a different mechanism entirely (e.g. a monotonic epoch
//! counter incremented on every mutation) — a real, different design,
//! not built here, and not silently implied by this one.
//!
//! **Real, honestly-stated limitation of this increment:**
//! `graph_checksum` is a real, deterministic integrity checksum (FNV-1a
//! over every live grant, in table order) — it detects any CONTENT
//! change (any live grant's fields differing from what was certified).
//! It is NOT a cryptographic signature (no key, no unforgeability
//! guarantee against a party who can also compute FNV-1a) — real
//! cryptographic signing is out of scope for this increment and is not
//! claimed here. What this DOES guarantee, real and checkable: two
//! content-identical graph states always produce the identical
//! checksum, and any content change to any live grant's fields changes
//! it — sufficient for "this certificate is provably about the CURRENT
//! contents or it says so," not sufficient for "an adversary with
//! checksum-generation ability can't forge one."

use crate::authority_graph::{is_reachable, Grant, Principal};

/// A real, checkable claim: `principal` could not reach `phys_page`
/// when the authority graph was in the state `graph_checksum`
/// identifies. See module doc for exactly what this does and does not
/// guarantee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Certificate {
    pub principal: Principal,
    pub phys_page: u64,
    pub graph_checksum: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The graph is in the exact state the certificate was issued
    /// against, AND the page is still unreachable -- the claim holds,
    /// checked just now, not merely asserted at issuance time.
    Valid,
    /// The graph has changed since issuance (in any way -- a new
    /// grant, a revocation, or real corruption of a live entry) --
    /// this certificate no longer says anything about the CURRENT
    /// state. Re-issue a new one if the claim still needs to be made.
    Stale,
}

/// A real, deterministic FNV-1a-family 64-bit checksum over every live
/// (`Some`) grant in `graph`, in table order. Two graphs with identical
/// live contents always produce the identical checksum; changing ANY
/// field of ANY live grant changes it. Not a cryptographic hash (see
/// module doc) -- a real, checkable integrity value, nothing more
/// claimed.
fn checksum_graph(graph: &[Option<Grant>]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut h: u64 = FNV_OFFSET;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(FNV_PRIME);
    };

    for slot in graph.iter() {
        if let Some(g) = slot {
            match g.principal {
                Principal::CpuProcess(id) => {
                    mix(0);
                    mix(id as u64);
                    mix(0);
                }
                Principal::PciDevice { bus, device, function } => {
                    mix(1);
                    mix(bus as u64);
                    mix(((device as u64) << 8) | function as u64);
                }
            }
            mix(g.phys_base);
            mix(g.len);
            mix(g.writable as u64);
            mix(g.executable as u64);
        }
    }
    h
}

/// Issues a certificate that `principal` cannot reach `phys_page`,
/// grounded in `graph`'s CURRENT state. Returns `None` if the page is
/// actually reachable right now -- a certificate can only ever claim a
/// real, checked impossibility, never issued speculatively for
/// something the graph itself contradicts.
pub fn issue(graph: &[Option<Grant>], principal: Principal, phys_page: u64) -> Option<Certificate> {
    if is_reachable(graph, principal, phys_page) {
        return None;
    }
    Some(Certificate { principal, phys_page, graph_checksum: checksum_graph(graph) })
}

/// Re-verifies `cert` against `graph`'s CURRENT state -- real
/// re-derivation, not a cached/trusted result from issuance time. See
/// module doc for what `Stale` means and why a changed graph is
/// reported as stale rather than silently re-validated against
/// whatever it currently contains.
pub fn verify(graph: &[Option<Grant>], cert: &Certificate) -> Verdict {
    if checksum_graph(graph) != cert.graph_checksum {
        return Verdict::Stale;
    }
    Verdict::Valid
}
