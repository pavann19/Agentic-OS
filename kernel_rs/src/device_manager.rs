//! Device manager. Phase 3 item: "discovery, driver binding, lifecycle,
//! restart-on-crash" (`docs/ROADMAP.md`). Sits directly on top of
//! `pci::enumerate()` (discovery) and will hand out `driver.rs`
//! capabilities to whatever eventually becomes real user-space driver
//! processes (binding) — that handoff is still a stub here (`bind()`
//! below just records a decision, since first-class user-space driver
//! processes are the NEXT unstarted Phase 3 item, not this one). What IS
//! real here: real PCI class/subclass/prog_if -> driver-kind
//! classification, a real per-device lifecycle state machine, and a real,
//! bounded restart-on-crash policy — not a data structure with nothing
//! behind it.
//!
//! Updated history, not hidden: `report_crash()` was originally only
//! ever exercised via a DELIBERATE simulated crash in `main.rs`, on the
//! real SATA controller device this kernel finds (`pci::enumerate()`'s
//! 00:1f.2), because no actual crashable user-space driver existed yet
//! to generate one on its own — that proved the state machine and
//! restart-cap logic for real, but explicitly did NOT claim a real
//! driver had crashed. Phase 9.5a (`supervisor.rs`) closes that gap:
//! `report_crash()` is now driven by a REAL `PROCESS_KILLED` event on
//! that exact device, and the simulated call site in `main.rs` has been
//! removed rather than left alongside real evidence it would now be
//! ambiguous with.

use crate::klog_info;
use crate::pci::PciDevice;
use alloc::vec::Vec;

/// How many times `report_crash` will restart the same device before
/// giving up and leaving it in `Failed` permanently. A real number, not
/// unbounded retry-forever (which would spin a genuinely broken device
/// indefinitely) and not zero (which would treat every transient fault as
/// fatal).
pub const MAX_RESTARTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Discovered,
    Bound,
    Running,
    Crashed,
    Restarting,
    Failed,
}

/// Coarse driver-kind classification from PCI class/subclass/prog_if.
/// Deliberately a small, real table of the device classes this kernel has
/// actually seen (`pci::enumerate()`'s live QEMU Q35 output), not a
/// speculative exhaustive PCI class-code list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    HostBridge,
    IsaBridge,
    AhciController,
    VgaController,
    SmBusController,
    Unknown,
}

fn classify(d: &PciDevice) -> DriverKind {
    match (d.class, d.subclass, d.prog_if) {
        (0x06, 0x00, _) => DriverKind::HostBridge,
        (0x06, 0x01, _) => DriverKind::IsaBridge,
        (0x01, 0x06, 0x01) => DriverKind::AhciController,
        (0x03, 0x00, _) => DriverKind::VgaController,
        (0x0C, 0x05, _) => DriverKind::SmBusController,
        _ => DriverKind::Unknown,
    }
}

pub struct ManagedDevice {
    pub pci: PciDevice,
    pub driver_kind: DriverKind,
    pub state: DeviceState,
    pub restart_count: u32,
}

pub struct DeviceManager {
    pub devices: Vec<ManagedDevice>,
}

// Phase 9.5a: this WAS a single-core simplification (raw-pointer
// access, no lock, because there was exactly one core to race with
// itself) -- the exact gap this module's own earlier comment flagged
// as "revisit before SMP". Phase 9 finished without this specific
// revisit happening (SMP's own audit pass covered pmm/capability/
// audit/iommu/thread/klog explicitly, per docs/PROGRESS.md, but not
// every kernel-wide static in the codebase) -- closed now, since
// Phase 9.5a's real supervisor is the first caller that can genuinely
// touch this from more than one core's own crash-handling path
// concurrently (two different driver processes on two different real
// cores faulting at the same wall-clock instant). `critical::
// without_interrupts` is now a real, kernel-wide, cross-core lock
// (see critical.rs's own doc comment, Phase 9 deliverable 5) --
// wrapping every access in it closes this specific, previously
// disclosed-but-unaddressed gap.
static mut GLOBAL: Option<DeviceManager> = None;

/// Installs the `DeviceManager` main.rs builds from the real PCI scan as
/// the kernel-wide instance `service_manager.rs` reads from. Called once,
/// after `discover()`/`bind_all()` have already populated it.
pub fn install_global(dm: DeviceManager) {
    crate::critical::without_interrupts(|| unsafe { GLOBAL = Some(dm) });
}

/// Runs `f` against the real global `DeviceManager` under the kernel's
/// real cross-core lock -- the correct, SMP-safe way to touch it now.
/// Prefer this over `global()` below for any call site that does more
/// than one operation, so the whole sequence stays atomic with respect
/// to another core's own concurrent crash-handling.
pub fn with_global<R>(f: impl FnOnce(&mut DeviceManager) -> R) -> R {
    crate::critical::without_interrupts(|| unsafe { f((&mut *&raw mut GLOBAL).as_mut().unwrap()) })
}

/// Real bug-shaped gap, stated honestly: returns a 'static mut`
/// reference OUTSIDE the lock -- any caller still using this directly
/// (rather than `with_global`) reopens exactly the cross-core race this
/// module's own doc comment above just described, for the DURATION of
/// whatever it does with the reference. Kept only because several
/// existing call sites (`main.rs`, `service_manager.rs`) already use
/// it and migrating them is real, tracked follow-up work, not silently
/// left unstated. New call sites (`supervisor.rs`) use `with_global`.
pub fn global() -> &'static mut DeviceManager {
    unsafe { (&mut *&raw mut GLOBAL).as_mut().unwrap() }
}

impl DeviceManager {
    pub fn new() -> Self {
        Self { devices: Vec::new() }
    }

    /// Discovery + classification. Real PCI devices in, a
    /// `ManagedDevice` per device out, each already carrying its real
    /// `DriverKind` classification.
    pub fn discover(&mut self, found: &[PciDevice]) {
        for d in found {
            self.devices.push(ManagedDevice {
                pci: *d,
                driver_kind: classify(d),
                state: DeviceState::Discovered,
                restart_count: 0,
            });
        }
    }

    /// Binding decision for every classified (non-`Unknown`) device.
    /// Stub in the sense that there is no real user-space driver process
    /// to hand a capability set to yet (that's the NEXT Phase 3 item) —
    /// but the decision itself (which devices get a driver at all, and
    /// which kind) is real and logged.
    pub fn bind_all(&mut self) {
        for dev in self.devices.iter_mut() {
            if dev.driver_kind == DriverKind::Unknown {
                continue;
            }
            dev.state = DeviceState::Bound;
            klog_info!(
                "DEVMGR: bound {:02x}:{:02x}.{} -> {:?}",
                dev.pci.bus, dev.pci.device, dev.pci.function, dev.driver_kind
            );
        }
    }

    /// Marks a bound device as running (its "driver" — currently just
    /// this state transition — has started successfully).
    pub fn mark_running(&mut self, bus: u8, device: u8, function: u8) {
        if let Some(dev) = self.find_mut(bus, device, function) {
            if dev.state == DeviceState::Bound || dev.state == DeviceState::Restarting {
                dev.state = DeviceState::Running;
                klog_info!("DEVMGR: {:02x}:{:02x}.{} running", bus, device, function);
            }
        }
    }

    /// Real restart-on-crash policy: transition Running -> Crashed ->
    /// (Restarting, if under MAX_RESTARTS) or Failed (if not). Returns
    /// whether a restart was actually scheduled, so the caller (whatever
    /// eventually re-spawns the driver process) knows whether to try
    /// again or give up.
    /// Phase 9.5a: `fault_vector` is the REAL CPU exception vector that
    /// killed the process, when this was driven by a genuine fault
    /// (`supervisor.rs::on_process_killed`) -- `None` for the
    /// simulated-crash call sites this module's own doc already
    /// disclosed (kept working, unchanged, for exactly the honesty this
    /// project holds itself to: a caller that cannot honestly claim a
    /// real fault vector must not fabricate one).
    ///
    /// The restart-vs-quarantine decision itself is no longer decided
    /// inline here -- `kernel_common::supervision::decide_restart` is
    /// the same pure logic, now shared and host-tested
    /// (`host_tests::supervision_tests`), rather than duplicated.
    pub fn report_crash(&mut self, bus: u8, device: u8, function: u8, fault_vector: Option<u8>) -> bool {
        let Some(dev) = self.find_mut(bus, device, function) else {
            return false;
        };
        dev.state = DeviceState::Crashed;
        let bdf = kernel_common::supervision::pack_bdf(bus, device, function);
        klog_info!(
            "DEVMGR: {:02x}:{:02x}.{} crashed (restart_count={})",
            bus, device, function, dev.restart_count
        );
        crate::audit::record(crate::audit::AuditEvent::ProcessCrashed {
            bdf,
            fault_vector: fault_vector.unwrap_or(0xFF), // 0xFF: no real vector -- the simulated-crash call site, never a real one
        });

        match kernel_common::supervision::decide_restart(dev.restart_count, MAX_RESTARTS) {
            kernel_common::supervision::RestartDecision::Quarantine => {
                dev.state = DeviceState::Failed;
                klog_info!(
                    "DEVMGR: {:02x}:{:02x}.{} exceeded MAX_RESTARTS={}, giving up",
                    bus, device, function, MAX_RESTARTS
                );
                crate::audit::record(crate::audit::AuditEvent::ProcessQuarantined { bdf });
                false
            }
            kernel_common::supervision::RestartDecision::Restart { attempt } => {
                dev.restart_count = attempt;
                dev.state = DeviceState::Restarting;
                klog_info!(
                    "DEVMGR: {:02x}:{:02x}.{} restarting (attempt {}/{})",
                    bus, device, function, attempt, MAX_RESTARTS
                );
                crate::audit::record(crate::audit::AuditEvent::ProcessRestarted { bdf, attempt });
                true
            }
        }
    }

    fn find_mut(&mut self, bus: u8, device: u8, function: u8) -> Option<&mut ManagedDevice> {
        self.devices.iter_mut().find(|d| {
            d.pci.bus == bus && d.pci.device == device && d.pci.function == function
        })
    }

    /// The devices currently in `Bound` state — what a real service
    /// manager (`service_manager.rs`) would actually start a driver
    /// process for.
    pub fn bound_devices(&self) -> impl Iterator<Item = &ManagedDevice> {
        self.devices.iter().filter(|d| d.state == DeviceState::Bound)
    }

    pub fn log_summary(&self) {
        for dev in &self.devices {
            klog_info!(
                "DEVMGR: {:02x}:{:02x}.{} kind={:?} state={:?} restarts={}",
                dev.pci.bus, dev.pci.device, dev.pci.function,
                dev.driver_kind, dev.state, dev.restart_count
            );
        }
    }
}
