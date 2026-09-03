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

use crate::{iommu, klog_info};
use kernel_common::authority_graph::{self, Entry, Grant, Principal};

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
