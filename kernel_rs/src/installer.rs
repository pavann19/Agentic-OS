//! Phase 13 deliverable 2 (`docs/ROADMAP.md` §5): "A package
//! store/installer service, itself an ordinary capability-holding
//! process — no special ambient install-time privilege." Deliberately
//! narrow, honest scope for this increment: what's real here is the
//! actual install-time DECISION logic (load this real ELF into a fresh
//! address space, grant it exactly the capabilities its own manifest
//! declares, refuse the rest) — the same real sequence every driver in
//! this kernel already repeats by hand (`ahci_driver_thread`,
//! `compositor_driver_thread`, `compositor::spawn_window_client`, ...),
//! now factored into one reusable function instead of copy-pasted per
//! driver. What is NOT here: reading a manifest+ELF pair off a real
//! on-disk package store — `object_store.rs`'s own real ext2 support
//! currently holds exactly one file (stated plainly in that module's
//! own doc), a real, pre-existing limitation this module does not
//! attempt to lift. Today's "installer" takes an already-in-memory ELF
//! and manifest, the same honest stand-in every other Phase 13/10 demo
//! in this session used for a piece of infrastructure that doesn't
//! exist yet (see `socket_demo.rs`'s own doc on this exact pattern).
//!
//! Further disclosed scope: `PortIoRange` (COM1-class port grants) is
//! covered below, reusing `driver.rs::create_port_capability`/
//! `grant_port_access` exactly as every driver's own hand-written spawn
//! code already does — real, not stubbed. `MmioRegion` and
//! `InterruptLine` are NOT: both need real, device-specific physical
//! parameters (a BAR's real physical address/size, a real IRQ vector)
//! that only the kernel-side code discovering that specific device
//! knows, not something a generic installer can decide on an app's
//! behalf — real, separate follow-up work, disclosed rather than
//! papered over with a fabricated default.

use crate::capability::{self, CapabilityTable, KernelObjectKind, ObjectId, Rights};
use crate::manifest::{CapKind, Manifest};
use crate::{audit, driver, klog_info, pmm, thread, vmm};

/// One capability an app is asking to be granted at install time — the
/// object it would name (constructed fresh, never reused across apps,
/// same "the kernel decides what exists, a process only receives what
/// it's granted" discipline every `create_*_capability` in this kernel
/// already follows) and the rights it wants on it. `label` is purely
/// for the log, so a real install-time denial is traceable to which
/// requested capability it was.
pub struct CapRequest {
    pub kind: CapKind,
    pub object_kind: KernelObjectKind,
    pub rights: Rights,
    pub label: &'static str,
}

/// Real core of the "installer" decision: loads `elf` into a FRESH
/// address space, maps a stack at `stack_vaddr`, and grants the calling
/// thread's OWN `cap_table` (`thread::grant_current_capability` — the
/// same real fix `compositor.rs`'s own doc describes for why a
/// throwaway local table would never actually reach the process about
/// to enter ring 3) exactly the requests whose `CapKind` `manifest`
/// declares. A request naming an undeclared kind is refused before its
/// object is even minted — no dangling `KernelObject` left behind for a
/// denied request, and a real `AuditEvent::ManifestDenied` record
/// either way. Returns the real ELF entry point and address space for
/// the caller to finish spawning with (kernel-stack setup, `syscall::
/// init`, `ring3::enter_user_mode` — these vary slightly per app, e.g.
/// whether an `INFO_VADDR` page of app-specific data is also mapped, so
/// they stay the caller's own responsibility rather than forced into
/// one rigid shape here).
pub fn install_into_current_thread(elf: &[u8], stack_vaddr: u64, manifest: Manifest, requests: &[CapRequest]) -> Option<(u64, u64)> {
    let space = unsafe { vmm::new_address_space() };
    let entry = match unsafe { crate::elf::load(space, elf) } {
        Ok(e) => e,
        Err(e) => {
            klog_info!("INSTALLER_ELF_LOAD_FAILED {:?}", e);
            return None;
        }
    };

    let stack_page = unsafe { pmm::alloc_page() };
    unsafe { vmm::map_page_in(space, stack_vaddr, stack_page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE) };

    for req in requests {
        if !manifest.declares(req.kind) {
            klog_info!("INSTALLER_GRANT_DENIED label={} kind={:?} -- not declared in this app's manifest", req.label, req.kind);
            audit::record(audit::AuditEvent::ManifestDenied { kind: req.kind as u8 });
            continue;
        }
        match req.object_kind {
            // Real, disclosed exception to the generic path below:
            // PortIoRange is enforced via the TSS IOPB
            // (`driver::grant_port_access`), never through the
            // process's own `cap_table` -- same real, deliberate split
            // every hand-written driver spawn already relies on
            // (`user_driver.rs::spawn_serial_driver`'s own throwaway
            // `CapabilityTable` used only to resolve the grant, never
            // stored anywhere afterward).
            KernelObjectKind::PortIoRange { base, count } => {
                let mut table = CapabilityTable::new();
                let cap = driver::create_port_capability(&mut table, base, count, req.rights);
                match driver::grant_port_access(&table, cap) {
                    Ok(()) => klog_info!("INSTALLER_GRANT_OK label={} kind={:?} base=0x{:x} count={}", req.label, req.kind, base, count),
                    Err(e) => klog_info!("INSTALLER_GRANT_FAILED label={} kind={:?} {:?}", req.label, req.kind, e),
                }
            }
            _ => {
                let object_id: ObjectId = capability::create_object(req.object_kind);
                let cap = thread::grant_current_capability(object_id, req.rights);
                klog_info!("INSTALLER_GRANT_OK label={} kind={:?} cap={}", req.label, req.kind, cap);
            }
        }
    }

    Some((entry, space))
}
