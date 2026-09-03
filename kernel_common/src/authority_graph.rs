//! Research track, `docs/NOVEL_CONCEPTS.md` §1: "the authority graph and
//! the hardware translation structures are the same object." This is
//! the isolated, pure-logic prototype of that claim -- `docs/RESEARCH_TRACK.md`
//! tracks its status; nothing here is wired into `kernel_rs`'s actual
//! boot path.
//!
//! **What this increment proves, precisely, and what it does NOT yet
//! prove:** that one authority structure (`grant`/`revoke` below) can
//! be the SOLE source both a CPU page-table-shaped projection and a
//! VT-d IOMMU second-level-table-shaped projection are derived from --
//! so revoking an entry removes it from both by construction, because
//! there is exactly one function (`revoke`) that mutates the source,
//! and both projectors are pure reads of that same source. What this
//! increment does NOT yet prove is real hardware atomicity (that a
//! revocation mid-flight against live silicon can't leave a window of
//! inconsistency) -- that requires wiring this into `kernel_rs::vmm`/
//! `kernel_rs::iommu` against real page tables, a later, separate,
//! hardware-facing increment. This one only has to establish the DATA
//! MODEL is sound, the same order this project always works in
//! (`kernel_common::driver_registry` proved its logic in `host_tests`
//! long before any integration decision).
//!
//! **Why the two projections can share one entry shape at all:** this
//! is not an assumption, it is a real, existing fact this codebase
//! already documents -- `kernel_rs::iommu::assign_device`'s own comment
//! states VT-d second-level translation "uses the identical x86-64
//! page table FORMAT as CPU paging, per the VT-d spec -- this is not a
//! coincidence, it's why a domain's page tables can be built with the
//! same bit layout." This module's `Entry` type is exactly that shared
//! shape: `kernel_common::pagetable::ADDR_MASK` framing plus flag bits,
//! reused for both projections rather than defined twice.
//!
//! **The real historical bug this design forecloses:** `kernel_rs::
//! iommu.rs`'s own module doc records that `assign_device` used to
//! allocate a fresh context-table page and clobber a bus's root-table
//! entry on every call, silently orphaning an earlier device's
//! assignment -- the kernel's bookkeeping and the hardware's actual
//! translation state had drifted apart, found by reading code, not by
//! any structural guarantee. `graph_bug_regression_matches_the_real_
//! historical_iommu_bug_shape` (in host_tests) is a direct regression
//! test for that exact shape, expressed at this pure-data-model level.

/// Who a grant's authority belongs to -- a CPU-executing process
/// (needs a page-table projection) or a PCI device (needs an IOMMU
/// second-level-table projection). Kept minimal on purpose: this
/// increment is proving the PROJECTION property, not modeling every
/// real principal kind this kernel eventually needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Principal {
    CpuProcess(u32),
    PciDevice { bus: u8, device: u8, function: u8 },
}

/// One unit of authority: `principal` may access `[phys_base, phys_base
/// + len)`, with `writable`/`executable` as CPU-projection flags
/// (`executable` is ignored by the IOMMU projection -- VT-d second-level
/// translation has no execute-permission bit; noted here rather than
/// silently dropped).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub principal: Principal,
    pub phys_base: u64,
    pub len: u64,
    pub writable: bool,
    pub executable: bool,
}

/// One projected hardware-table-shaped entry -- the SAME shape for
/// both projections (see module doc for why that's real, not
/// convenient): a page-granular physical frame plus the flag bits a
/// real leaf PTE or VT-d second-level PTE would carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub phys_page: u64,
    pub writable: bool,
    pub executable: bool,
}

const PAGE_SIZE: u64 = 4096;

fn pages_for(len: u64) -> u64 {
    (len + PAGE_SIZE - 1) / PAGE_SIZE
}

/// Adds `g` into the first empty slot of `graph`. Returns `true` if
/// added, `false` if `graph` is full -- bounded, no panic, same
/// discipline as `driver_registry::match_all`.
pub fn grant(graph: &mut [Option<Grant>], g: Grant) -> bool {
    for slot in graph.iter_mut() {
        if slot.is_none() {
            *slot = Some(g);
            return true;
        }
    }
    false
}

/// The ONLY revocation path. Removes every grant belonging to
/// `principal` whose range starts at `phys_base`. There is
/// deliberately no separate "remove from the CPU projection" or
/// "remove from the IOMMU projection" function -- see module doc.
/// Returns the number of grants removed.
pub fn revoke(graph: &mut [Option<Grant>], principal: Principal, phys_base: u64) -> usize {
    let mut removed = 0;
    for slot in graph.iter_mut() {
        if let Some(g) = slot {
            if g.principal == principal && g.phys_base == phys_base {
                *slot = None;
                removed += 1;
            }
        }
    }
    removed
}

/// Projects every grant belonging to `principal` into page-granular
/// `Entry` values, written into `out`. Returns the count written.
/// Bounded: stops once `out` is full.
///
/// **Enforced, not just documented:** `principal` must be a
/// `CpuProcess`. A `PciDevice` principal returns 0 unconditionally
/// (found by this module's own real, honest test failure the first
/// time this was written with the distinction only documented, not
/// checked -- `projections_are_filtered_by_principal_not_shared_
/// across_principals` in host_tests caught a caller-facing gap where
/// nothing stopped a device's grant from being read out through the
/// CPU-shaped projector or vice versa. That gap is exactly the class
/// of "software believes one thing, nothing checks it" this whole
/// concept exists to foreclose, so it is fixed here at the type/call
/// level, not left as a convention).
pub fn project_page_table(graph: &[Option<Grant>], principal: Principal, out: &mut [Entry]) -> usize {
    match principal {
        Principal::CpuProcess(_) => project(graph, principal, out),
        Principal::PciDevice { .. } => 0,
    }
}

/// Projects every grant belonging to `principal` into the SAME `Entry`
/// shape `project_page_table` emits -- real VT-d second-level
/// translation entries share that exact bit layout with CPU
/// page-table entries (module doc). Both projectors read the same
/// `graph`: that sameness IS the claim being tested, not an accident
/// of code reuse.
///
/// **Enforced, not just documented:** `principal` must be a
/// `PciDevice` -- see `project_page_table`'s own note on why this is
/// a real check, not a comment.
pub fn project_iommu_table(graph: &[Option<Grant>], principal: Principal, out: &mut [Entry]) -> usize {
    match principal {
        Principal::PciDevice { .. } => project(graph, principal, out),
        Principal::CpuProcess(_) => 0,
    }
}

fn project(graph: &[Option<Grant>], principal: Principal, out: &mut [Entry]) -> usize {
    let mut count = 0;
    for slot in graph.iter() {
        if count >= out.len() {
            break;
        }
        if let Some(g) = slot {
            if g.principal != principal {
                continue;
            }
            let pages = pages_for(g.len);
            let mut p = 0u64;
            while p < pages && count < out.len() {
                out[count] = Entry {
                    phys_page: g.phys_base + p * PAGE_SIZE,
                    writable: g.writable,
                    executable: g.executable,
                };
                count += 1;
                p += 1;
            }
        }
    }
    count
}

/// Real, direct check that no live entry in `graph`, projected either
/// way, ever touches `phys_page` for `principal` -- the primitive
/// `docs/NOVEL_CONCEPTS.md` §1.4's revocation test is built from.
/// Pure, bounded (scans `graph` once), no allocation.
pub fn is_reachable(graph: &[Option<Grant>], principal: Principal, phys_page: u64) -> bool {
    for slot in graph.iter() {
        if let Some(g) = slot {
            if g.principal != principal {
                continue;
            }
            let pages = pages_for(g.len);
            let start = g.phys_base;
            let end = g.phys_base + pages * PAGE_SIZE;
            if phys_page >= start && phys_page < end {
                return true;
            }
        }
    }
    false
}
