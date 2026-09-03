//! Research track (`docs/RESEARCH_TRACK.md`, `docs/NOVEL_CONCEPTS.md`
//! §3): least privilege discovered by observed access, then frozen
//! into the authority graph as an enforced envelope — instead of a
//! human or a policy engine declaring what a component *should* need,
//! this derives the envelope from what it was actually, really
//! observed touching. Isolated, pure-logic prototype, rated only
//! MODERATE novelty confidence in `docs/NOVEL_CONCEPTS.md` §4 (real
//! adjacent art: systrace/AppArmor/seccomp learning-mode profile
//! generation, named there rather than glossed over) — built anyway
//! because it is a real, cheap, useful consequence of §1 existing, not
//! because the novelty claim alone would justify it.
//!
//! **Real, stated scope of this increment:** this operates on a
//! caller-provided list of physical-page ACCESS ATTEMPTS (`u64`
//! addresses) — it does not itself observe real IOMMU/MMU faults.
//! Wiring it to REAL hardware fault events (`kernel_rs::iommu::
//! poll_and_log_faults`'s own fault records, replacing the synthetic
//! attempt list this module's tests use with a real captured fault
//! trace from a real driver run) is real, separate follow-up work, not
//! done here — same "prove the data model, then wire it to silicon"
//! order every increment in this research track has followed.

use crate::authority_graph::{self, Grant, Principal};

/// Deduplicates `attempts` (real or simulated physical-page addresses
/// a component touched) into `out`, in first-seen order. Returns the
/// count written. Bounded: stops once `out` is full, same no-silent-
/// overrun discipline as every other `kernel_common` output-slice
/// function.
pub fn discover_envelope(attempts: &[u64], out: &mut [u64]) -> usize {
    let mut count = 0;
    for &addr in attempts {
        if count >= out.len() {
            break;
        }
        let mut already_seen = false;
        for i in 0..count {
            if out[i] == addr {
                already_seen = true;
                break;
            }
        }
        if !already_seen {
            out[count] = addr;
            count += 1;
        }
    }
    count
}

/// "Freezes" a discovered envelope into the authority graph as real,
/// enforced grants -- one page-granular `Grant` per address in
/// `envelope`, for `principal`. Returns the number actually granted
/// (bounded by both `envelope`'s own length and the graph's own
/// capacity — `authority_graph::grant`'s existing bounded-no-panic
/// contract, not re-implemented here).
pub fn freeze_envelope(graph: &mut [Option<Grant>], principal: Principal, envelope: &[u64]) -> usize {
    let mut granted = 0;
    for &phys_page in envelope {
        let g = Grant { principal, phys_base: phys_page, len: 4096, writable: true, executable: false };
        if authority_graph::grant(graph, g) {
            granted += 1;
        }
    }
    granted
}
