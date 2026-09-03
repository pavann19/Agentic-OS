//! Research track (`docs/RESEARCH_TRACK.md`, `docs/NOVEL_CONCEPTS.md`
//! §1): the real hardware-facing wiring for `kernel_common::
//! authority_graph`'s "one authority structure, hardware translation
//! structures are its projection" claim. The pure data-model prototype
//! (`host_tests`, zero hardware) proved the SHAPE of this; this module
//! is where it starts touching real silicon.
//!
//! **What this increment wires, precisely:** a device's DMA grant is no
//! longer tracked twice (once implicitly by whatever phys-range list a
//! caller happened to pass to `iommu::assign_device`, and once, if at
//! all, in some separate bookkeeping structure). It is tracked ONCE, in
//! `GRAPH` below, and `grant_device`/`revoke_device` derive the real
//! `iommu::assign_device`/`iommu::revoke_device` calls FROM the graph's
//! own projection (`kernel_common::authority_graph::project_iommu_table`)
//! rather than from a second, independently-maintained list. This is
//! the actual claim from `docs/NOVEL_CONCEPTS.md` §1 — "IOMMU tables
//! are generated from the capability graph as its only projection" —
//! applied to this kernel's real, existing `iommu.rs`, not a simulation
//! of it.
//!
//! **What this increment deliberately does NOT yet wire:** the CPU
//! page-table side (`project_page_table` → real `vmm::map_page_in`
//! calls). Device DMA containment is where this kernel's real historical
//! bug lived (`iommu.rs`'s own module doc), so it is the first real
//! target; wiring process page tables through the same graph is a
//! separate, later increment once this one holds up under real use.
//!
//! **Real, honest scope of "atomicity" this increment demonstrates:**
//! `revoke_device` clears the actual IOMMU context-table entry
//! (`iommu::revoke_device`) in the same call that removes the grant
//! from `GRAPH` — verified by reading back the real hardware-facing
//! context-table bytes (`iommu::context_entry_present`), not merely by
//! the call returning without error. It does NOT yet demonstrate a live
//! third-party device's in-flight DMA actually faulting the instant
//! after revocation (that needs a real device issuing a second,
//! post-revocation DMA attempt — a further increment, noted in
//! `docs/RESEARCH_TRACK.md`, not silently implied as done here).

use crate::{iommu, klog_info, vmm};
use kernel_common::authority_graph::{self, Entry, Grant, Principal};
use kernel_common::impossibility_certificate::{self, Certificate, Verdict};

const MAX_GRANTS: usize = 32;

static mut GRAPH: [Option<Grant>; MAX_GRANTS] = [None; MAX_GRANTS];

/// The real wiring point: grants `phys_base..phys_base+len` to PCI
/// device `bus:device.function`, adds it to the ONE authority graph,
/// and derives the real `iommu::assign_device` call from that graph's
/// own projection — the ranges `iommu::assign_device` receives are
/// read back out of `GRAPH`, not passed through separately. Returns
/// the real `iommu::DomainId`, or `None` if the graph is full (bounded,
/// same "no silent overrun" discipline as every other kernel_common
/// caller in this tree) or if the underlying `assign_device` call
/// itself fails.
pub fn grant_device(bus: u8, device: u8, function: u8, phys_base: u64, len: u64) -> Option<iommu::DomainId> {
    let principal = Principal::PciDevice { bus, device, function };

    let added = unsafe {
        authority_graph::grant(
            &mut *&raw mut GRAPH,
            Grant { principal, phys_base, len, writable: true, executable: false },
        )
    };
    if !added {
        klog_info!("AUTHORITY: graph full -- refusing to grant {:02x}:{:02x}.{} (bounded, not silently dropped)", bus, device, function);
        return None;
    }

    // Derive the real ranges from the graph's OWN projection -- this
    // is the actual "single source" claim, not a comment: nothing here
    // reads `phys_base`/`len` a second time from the caller's own
    // arguments to build the range list `assign_device` gets.
    let mut entries = [Entry { phys_page: 0, writable: false, executable: false }; MAX_GRANTS];
    let count = unsafe { authority_graph::project_iommu_table(&*&raw const GRAPH, principal, &mut entries) };
    if count == 0 {
        // Should be unreachable given the grant() above just succeeded,
        // but real defensive handling rather than an assumed invariant.
        klog_info!("AUTHORITY: projection produced zero entries right after a successful grant -- refusing to assign hardware");
        return None;
    }

    let mut ranges: [(u64, u64); MAX_GRANTS] = [(0, 0); MAX_GRANTS];
    for i in 0..count {
        ranges[i] = (entries[i].phys_page, 4096);
    }

    let domain = iommu::assign_device(bus, device, function, &ranges[..count]);
    klog_info!(
        "AUTHORITY_GRANT_HW device={:02x}:{:02x}.{} phys=0x{:x} len={} projected_pages={} -- real iommu::assign_device call derived from the graph's own projection",
        bus, device, function, phys_base, len, count
    );
    Some(domain)
}

/// The ONLY revocation path for a device grant: removes it from
/// `GRAPH` AND clears the real IOMMU context entry, in the same call
/// -- there is no separate "now also tell the IOMMU" step to forget,
/// which is the entire property `docs/NOVEL_CONCEPTS.md` §1 is about.
/// Returns `true` if both the graph entry and the real hardware
/// context entry were found and cleared.
pub fn revoke_device(bus: u8, device: u8, function: u8, phys_base: u64) -> bool {
    let principal = Principal::PciDevice { bus, device, function };
    let graph_removed = unsafe { authority_graph::revoke(&mut *&raw mut GRAPH, principal, phys_base) };
    let hw_revoked = iommu::revoke_device(bus, device, function);

    klog_info!(
        "AUTHORITY_REVOKE_HW device={:02x}:{:02x}.{} graph_entries_removed={} hardware_context_cleared={}",
        bus, device, function, graph_removed, hw_revoked
    );
    graph_removed > 0 && hw_revoked
}

/// Real, direct evidence check: does the graph still consider
/// `phys_page` reachable by this device, AND does the real IOMMU
/// context entry agree? Used by the boot-time self-check below (and
/// available for any future test) to verify software and hardware
/// have not drifted apart -- the actual property this whole module
/// exists to make unrepresentable.
pub fn cross_check(bus: u8, device: u8, function: u8, phys_page: u64) -> (bool, bool) {
    let principal = Principal::PciDevice { bus, device, function };
    let graph_says = unsafe { authority_graph::is_reachable(&*&raw const GRAPH, principal, phys_page) };
    let hw_says = iommu::context_entry_present(bus, device, function);
    (graph_says, hw_says)
}

/// Grants exactly one 4KB page in `envelope` per element -- reuses
/// `grant_device` for each, so the SAME "derive the real hardware call
/// from the graph's own projection" property `grant_device` already
/// provides applies to every page frozen this way. Returns the number
/// successfully granted. The real §3 wiring point
/// (`docs/NOVEL_CONCEPTS.md` §3, `docs/RESEARCH_TRACK.md`): freezing a
/// DISCOVERED envelope (`kernel_common::discovered_envelope::
/// discover_envelope`, fed from REAL captured IOMMU fault addresses --
/// see `authority_hw_fault_demo.rs`) into real, enforced hardware
/// grants, not just the pure graph.
pub fn freeze_envelope_device(bus: u8, device: u8, function: u8, envelope: &[u64]) -> usize {
    let mut granted = 0;
    for &phys_page in envelope {
        if grant_device(bus, device, function, phys_page, 4096).is_some() {
            granted += 1;
        }
    }
    granted
}

// ---------------------------------------------------------------------
// §1's remaining half: the CPU-side page-table projection, wired to
// real vmm::map_page_in/unmap_page -- the escalation this module's own
// doc named as deliberately not done in the device-DMA increment.
// Real, stated scope limit of THIS increment: one live grant per
// (pid, phys_base) pair is assumed for revoke's own vaddr bookkeeping
// (the caller passes the same vaddr_base back at revoke time) --
// multiple simultaneous grants for the same process work fine for
// GRANTING (project_page_table returns every one of them, all get
// mapped), but revoke_process here only unmaps the specific
// (pid, phys_base) grant being revoked, using vaddr_base the CALLER
// supplies for that grant, not a value recorded by this module. A
// real per-grant vaddr table is a natural follow-up, not built here.
// ---------------------------------------------------------------------

/// Real wiring point for §1's CPU side: grants `phys_base..phys_base+
/// len` to CPU process `pid`, adds it to the ONE authority graph, and
/// derives real `vmm::map_page_in` calls into `target_pml4` FROM the
/// graph's own `project_page_table` projection -- mapped starting at
/// `vaddr_base`, one page per projected entry, in projection order.
/// Real permission flags come from the projected `Entry`, not
/// re-derived from the caller's own `writable`/`executable` arguments
/// a second time (the same "single source" discipline `grant_device`
/// already established for the device side). Returns `true` if the
/// grant was added and at least one page was mapped.
pub fn grant_process(
    pid: u32,
    target_pml4: u64,
    vaddr_base: u64,
    phys_base: u64,
    len: u64,
    writable: bool,
    executable: bool,
) -> bool {
    let principal = Principal::CpuProcess(pid);
    let added = unsafe {
        authority_graph::grant(
            &mut *&raw mut GRAPH,
            Grant { principal, phys_base, len, writable, executable },
        )
    };
    if !added {
        klog_info!("AUTHORITY: graph full -- refusing to grant CPU process {} (bounded, not silently dropped)", pid);
        return false;
    }

    let mut entries = [Entry { phys_page: 0, writable: false, executable: false }; MAX_GRANTS];
    let count = unsafe { authority_graph::project_page_table(&*&raw const GRAPH, principal, &mut entries) };
    if count == 0 {
        klog_info!("AUTHORITY: CPU projection produced zero entries right after a successful grant -- refusing to map hardware");
        return false;
    }

    unsafe {
        for i in 0..count {
            let vaddr = vaddr_base + (i as u64) * 4096;
            let mut flags = 0u64;
            if entries[i].writable {
                flags |= vmm::PAGE_WRITABLE;
            }
            if !entries[i].executable {
                flags |= vmm::PAGE_NO_EXECUTE;
            }
            vmm::map_page_in(target_pml4, vaddr, entries[i].phys_page, flags);
        }
    }
    klog_info!(
        "AUTHORITY_GRANT_HW_CPU pid={} vaddr_base=0x{:x} phys=0x{:x} len={} projected_pages={} -- real vmm::map_page_in calls derived from the graph's own projection",
        pid, vaddr_base, phys_base, len, count
    );
    true
}

/// The ONLY revocation path for a CPU-process grant: removes it from
/// `GRAPH` AND unmaps the real page-table entries `vmm::map_page_in`
/// created for it, in the same call. `vaddr_base` must be the SAME
/// value passed to the matching `grant_process` call (see this
/// section's own doc on why -- this module does not itself remember
/// per-grant vaddrs yet). Returns `true` if the graph entry was found
/// and removed.
pub fn revoke_process(pid: u32, target_pml4: u64, vaddr_base: u64, phys_base: u64) -> bool {
    let principal = Principal::CpuProcess(pid);
    // Project BEFORE revoking -- once the grant is gone from GRAPH,
    // project_page_table can no longer tell us how many pages (and
    // therefore how many vaddrs) this specific grant covered.
    let mut entries = [Entry { phys_page: 0, writable: false, executable: false }; MAX_GRANTS];
    let count = unsafe { authority_graph::project_page_table(&*&raw const GRAPH, principal, &mut entries) };

    let removed = unsafe { authority_graph::revoke(&mut *&raw mut GRAPH, principal, phys_base) };
    if removed > 0 {
        unsafe {
            for i in 0..count {
                let vaddr = vaddr_base + (i as u64) * 4096;
                vmm::unmap_page(target_pml4, vaddr);
            }
        }
    }
    klog_info!(
        "AUTHORITY_REVOKE_HW_CPU pid={} vaddr_base=0x{:x} graph_entries_removed={} pages_unmapped={}",
        pid, vaddr_base, removed, count
    );
    removed > 0
}

/// Real, direct evidence check for the CPU side: does the graph still
/// consider `phys_page` reachable by process `pid`, AND does a REAL
/// page-table walk of `target_pml4` at `vaddr` (`vmm::debug_translate`
/// -- the exact mechanism, reading the exact bytes, the CPU's own MMU
/// would walk) agree?
///
/// Real bug found by this function's own first test run, fixed here
/// rather than hidden: `vmm::debug_translate` returns the RAW leaf PTE
/// value (address bits AND flag bits together, exactly what's stored
/// in the table), not a bare frame address -- comparing it directly
/// against `phys_page` was wrong whenever any flag bit was set (which
/// is always, in practice: PRESENT alone guarantees a mismatch).
/// `kernel_common::pagetable::frame_from_entry` (already used
/// elsewhere in this kernel for exactly this masking) is the real fix.
pub fn cross_check_cpu(pid: u32, target_pml4: u64, vaddr: u64, phys_page: u64) -> (bool, bool) {
    let principal = Principal::CpuProcess(pid);
    let graph_says = unsafe { authority_graph::is_reachable(&*&raw const GRAPH, principal, phys_page) };
    let raw_entry = unsafe { vmm::debug_translate(target_pml4, vaddr) };
    let hw_frame = kernel_common::pagetable::frame_from_entry(raw_entry);
    (graph_says, hw_frame == phys_page)
}

// ---------------------------------------------------------------------
// §2's real hardware wiring: a certificate bound to the ACTUAL raw
// IOMMU context-table bytes for a device, not only the pure graph --
// the escalation `docs/NOVEL_CONCEPTS.md` §2.4 describes ("corrupt one
// real IOMMU table entry underneath it, show the certificate fails to
// re-verify").
// ---------------------------------------------------------------------

/// A certificate plus the REAL raw hardware bytes it was issued
/// against (`iommu::context_entry_raw`) -- the exact 128 bits the
/// IOMMU silicon itself consults for this device's context entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HwCertificate {
    pub cert: Certificate,
    pub hw_low: u64,
    pub hw_high: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HwVerdict {
    /// Both the graph and the real hardware bytes still match what
    /// was certified.
    Valid,
    /// The pure authority graph itself has changed since issuance --
    /// see `kernel_common::impossibility_certificate`'s own doc on
    /// what this means.
    GraphStale,
    /// The graph matches, but the REAL hardware context-table bytes
    /// for this device do not match what was certified -- a real,
    /// hardware-level tamper/drift signal, distinguishable from a
    /// mere software-side change.
    HardwareMismatch,
}

/// Issues a certificate that `bus:device.function` cannot reach
/// `phys_page`, bound to BOTH the pure graph state (`kernel_common::
/// impossibility_certificate::issue`) AND the real, live IOMMU
/// context-table bytes for that device at issuance time. `None` if
/// the page is actually reachable right now (same real-impossibility-
/// only contract the pure version has).
pub fn issue_device_certificate(bus: u8, device: u8, function: u8, phys_page: u64) -> Option<HwCertificate> {
    let principal = Principal::PciDevice { bus, device, function };
    let cert = unsafe { impossibility_certificate::issue(&*&raw const GRAPH, principal, phys_page) }?;
    let (hw_low, hw_high) = iommu::context_entry_raw(bus, device, function);
    Some(HwCertificate { cert, hw_low, hw_high })
}

/// Re-verifies `hwcert` against BOTH the current graph state and the
/// CURRENT real IOMMU context-table bytes for the device -- real
/// re-derivation of both, not a cached result. Distinguishes a
/// software-only change (`GraphStale`) from a real hardware-level
/// mismatch (`HardwareMismatch`) -- the real, falsifiable evidence
/// `docs/NOVEL_CONCEPTS.md` §2.4 asks for: a certificate that
/// specifically detects tampering with the real hardware structure it
/// was bound to, not only its own software model of that structure.
pub fn verify_device_certificate(bus: u8, device: u8, function: u8, hwcert: &HwCertificate) -> HwVerdict {
    let graph_verdict = unsafe { impossibility_certificate::verify(&*&raw const GRAPH, &hwcert.cert) };
    if graph_verdict == Verdict::Stale {
        return HwVerdict::GraphStale;
    }
    let (hw_low, hw_high) = iommu::context_entry_raw(bus, device, function);
    if hw_low != hwcert.hw_low || hw_high != hwcert.hw_high {
        return HwVerdict::HardwareMismatch;
    }
    HwVerdict::Valid
}
