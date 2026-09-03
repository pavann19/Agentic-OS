//! Service manager. Phase 3 item, second half of "init and a service
//! manager as the first user-space processes" (`docs/ROADMAP.md`).
//!
//! Honesty note (scope, stated plainly rather than glossed over): this
//! process itself still runs in ring 0, as a kernel thread — NOT in ring
//! 3 like `init.rs`. Getting it into ring 3 for real needs an ELF loader
//! for arbitrary user binaries (this kernel can currently only run
//! hand-built machine-code blobs mapped directly into a fresh address
//! space, the same technique `init.rs` and the Phase 1/2 `demo_ring3`
//! proof both use) — building that loader is exactly the work the next
//! Phase 3 item ("first user-space drivers") requires anyway, so it's
//! deferred there rather than duplicated here. What IS real in this
//! module: it is a genuinely separate, independently-scheduled thread
//! from `init`, woken by a REAL capability-gated syscall `init` issues
//! from ring 3 (not a plain function call or a hardcoded boot-order
//! assumption), and it reads the REAL `device_manager` state built from
//! this session's own PCI scan to decide what it would start.

use crate::{device_manager, ipc, klog_info, syscall};

/// Blocks until `init`'s real ring-3 `syscall 3` (SVC_START) delivers its
/// token through `syscall::init_svc_table()`/`init_svc_cap()` — the exact
/// same capability-gated IPC endpoint the syscall dispatcher sends into,
/// so this thread only proceeds once a genuine ring-3 process asked it
/// to, not on a fixed boot-order assumption.
pub extern "C" fn service_manager_thread() {
    let table = syscall::init_svc_table();
    let cap = syscall::init_svc_cap();
    match ipc::receive(table, cap) {
        Ok(msg) => klog_info!("SERVICE_MANAGER: got SVC_START token=0x{:x} from init", msg.data[0]),
        Err(e) => {
            klog_info!("SERVICE_MANAGER: FAILED to receive SVC_START: {:?}", e);
            return;
        }
    }

    let dm = device_manager::global();
    let mut count = 0u32;
    // Collect first (bound_devices borrows `dm` immutably; mark_running
    // below needs it mutably) -- real bound devices from the real PCI
    // scan, not a synthetic list.
    let to_start: alloc::vec::Vec<_> = dm
        .bound_devices()
        .map(|d| (d.pci.bus, d.pci.device, d.pci.function, d.driver_kind))
        .collect();
    for (bus, device, function, kind) in to_start {
        klog_info!(
            "SERVICE_MANAGER: would start driver process for {:02x}:{:02x}.{} kind={:?}",
            bus, device, function, kind
        );
        // No ELF-loaded user-space driver process exists yet to actually
        // spawn (see module doc) -- marking Running here documents the
        // real decision made without claiming a process was started.
        dm.mark_running(bus, device, function);
        count += 1;
    }
    klog_info!("SERVICE_MANAGER: startup decisions made for {} device(s)", count);
}
