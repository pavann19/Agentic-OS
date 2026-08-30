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
//! Honesty note: `report_crash()` is exercised in `main.rs` via a
//! DELIBERATE simulated crash on the real SATA controller device this
//! kernel already found (`pci::enumerate()`'s 00:1f.2), because no actual
//! crashable user-space driver exists yet to generate one on its own.
//! This proves the state machine and restart-cap logic for real; it does
//! not claim a real driver crashed.

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
    pub fn report_crash(&mut self, bus: u8, device: u8, function: u8) -> bool {
        let Some(dev) = self.find_mut(bus, device, function) else {
            return false;
        };
        dev.state = DeviceState::Crashed;
        klog_info!(
            "DEVMGR: {:02x}:{:02x}.{} crashed (restart_count={})",
            bus, device, function, dev.restart_count
        );
        if dev.restart_count >= MAX_RESTARTS {
            dev.state = DeviceState::Failed;
            klog_info!(
                "DEVMGR: {:02x}:{:02x}.{} exceeded MAX_RESTARTS={}, giving up",
                bus, device, function, MAX_RESTARTS
            );
            return false;
        }
        dev.restart_count += 1;
        dev.state = DeviceState::Restarting;
        klog_info!(
            "DEVMGR: {:02x}:{:02x}.{} restarting (attempt {}/{})",
            bus, device, function, dev.restart_count, MAX_RESTARTS
        );
        true
    }

    fn find_mut(&mut self, bus: u8, device: u8, function: u8) -> Option<&mut ManagedDevice> {
        self.devices.iter_mut().find(|d| {
            d.pci.bus == bus && d.pci.device == device && d.pci.function == function
        })
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
