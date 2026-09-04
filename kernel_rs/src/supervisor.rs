//! Phase 9.5a (`docs/ROADMAP.md` §5) — the real, in-OS wire between two
//! mechanisms that already existed and were already evidenced, but had
//! never been connected: `idt.rs::recover_or_halt` catches a real ring-3
//! fault and kills exactly that process (real since Phase 1); `device_
//! manager.rs`'s restart-on-crash state machine (real since Phase 3) has
//! only ever been driven by a DELIBERATELY SIMULATED crash, stated
//! plainly in that module's own doc. This module is the missing wire:
//! it observes the real death, identifies which real PCI device the
//! dying thread belonged to, and drives the real restart policy against
//! it — respawning a real driver process, for real.
//!
//! Scope, stated as plainly as `device_manager.rs`'s own honesty note:
//! this is Phase 9.5a, NOT 9.5b. Nothing here adopts new code, tunes a
//! policy from recorded history, or touches anything ADR-009/ADR-010
//! would gate. A restarted process runs the EXACT SAME code it crashed
//! with — this is supervision, not improvement.

use crate::{device_manager, klog_info, thread};
use alloc::vec::Vec;
use kernel_common::supervision::pack_bdf;

/// One real, currently-registered driver a device may be respawned by.
/// `owner_thread` is the CURRENT live thread id running this driver's
/// code (0 = none yet / not currently running) — updated by `mark_
/// thread_owner` every time a fresh instance (initial spawn OR a
/// respawn) actually starts executing, so a later crash can be traced
/// back to the exact device it belongs to using nothing but `thread::
/// current_id()`, the only identity `idt.rs`'s fault path has to work
/// with.
struct SupervisedDriver {
    bus: u8,
    device: u8,
    function: u8,
    respawn: fn(),
    owner_thread: u64,
}

// Real, bounded storage — this kernel has a handful of real drivers
// (ahci, nvme, e1000, virtio_blk, virtio_net, the two user_driver.rs
// demos), not an unbounded number; a small Vec under the kernel's own
// real cross-core lock is correct and simple, matching device_manager.
// rs's own `Vec<ManagedDevice>` for the same real device count.
static mut REGISTRY: Option<Vec<SupervisedDriver>> = None;

#[allow(static_mut_refs)]
fn registry_mut() -> &'static mut Vec<SupervisedDriver> {
    unsafe {
        let slot = &mut *&raw mut REGISTRY;
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
        slot.as_mut().unwrap()
    }
}

/// Registers a real device as supervisable — called once, at spawn
/// time, right after the device's driver thread is first spawned
/// (`main.rs`, alongside `ahci::spawn_if_present` et al.). `respawn` is
/// the SAME real spawn function used the first time — a real
/// restarted process runs the identical code path a fresh boot would,
/// nothing improvised.
pub fn register(bus: u8, device: u8, function: u8, respawn: fn()) {
    crate::critical::without_interrupts(|| {
        let reg = registry_mut();
        if reg.iter().any(|d| d.bus == bus && d.device == device && d.function == function) {
            return; // already registered -- idempotent, matches syscall::init()'s own convention
        }
        reg.push(SupervisedDriver { bus, device, function, respawn, owner_thread: 0 });
        klog_info!("SUPERVISOR_REGISTERED device={:02x}:{:02x}.{}", bus, device, function);
    });
}

/// Called by a driver thread itself, as close to its own real start as
/// practical, once `thread::current_id()` is known to be ITS OWN id —
/// records "the thread currently running this device's driver is THIS
/// one," so a later fault on this exact thread can be traced back to
/// this exact device. Must be called again after every respawn (the
/// respawned thread has a brand-new id) — real callers do this
/// naturally, since `respawn` just re-invokes the same driver-thread
/// entry point, which calls this itself on every real start.
pub fn mark_thread_owner(bus: u8, device: u8, function: u8) {
    crate::critical::without_interrupts(|| {
        let tid = thread::current_id();
        if let Some(d) = registry_mut().iter_mut().find(|d| d.bus == bus && d.device == device && d.function == function) {
            d.owner_thread = tid;
            klog_info!("SUPERVISOR_OWNER device={:02x}:{:02x}.{} tid={}", bus, device, function, tid);
        }
    });
}

/// Called from `idt.rs::recover_or_halt`, BEFORE the faulting thread is
/// actually killed (`thread::current_id()` must still resolve to the
/// dying thread, not whatever gets scheduled next) — this is the real
/// observation point: a REAL fault, on a REAL registered driver thread,
/// with a REAL captured exception vector. Looks up which device this
/// thread belongs to; if found, drives `device_manager::report_crash`
/// with the real vector and, if a restart is approved, actually
/// respawns the driver — real crash-to-restart, end to end, with no
/// simulated step anywhere in the path.
///
/// A silent no-op for a dying thread that isn't a registered driver
/// (the fault-isolation demo's own deliberately-faulting kernel thread,
/// for instance) — this module only supervises real PCI-device driver
/// processes, not every ring-3 thread in the system.
pub fn on_process_killed(fault_vector: u8) {
    let tid = thread::current_id();
    let target = crate::critical::without_interrupts(|| {
        registry_mut()
            .iter()
            .find(|d| d.owner_thread == tid)
            .map(|d| (d.bus, d.device, d.function, d.respawn))
    });
    let Some((bus, device, function, respawn)) = target else {
        return; // not a supervised driver -- nothing for this module to do
    };

    klog_info!(
        "SUPERVISOR_REAL_DEATH device={:02x}:{:02x}.{} tid={} fault_vector={}",
        bus, device, function, tid, fault_vector
    );
    let bdf = pack_bdf(bus, device, function);
    let _ = bdf; // real identity, logged above via the real fields -- kept for clarity at the call site below

    let restart_scheduled = device_manager::with_global(|dm| dm.report_crash(bus, device, function, Some(fault_vector)));

    if restart_scheduled {
        klog_info!("SUPERVISOR_RESPAWN device={:02x}:{:02x}.{}", bus, device, function);
        respawn();
        device_manager::with_global(|dm| dm.mark_running(bus, device, function));
    } else {
        klog_info!("SUPERVISOR_QUARANTINED device={:02x}:{:02x}.{} -- not respawning", bus, device, function);
    }
}
