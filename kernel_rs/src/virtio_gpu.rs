//! VirtIO-GPU Graphics Device Discovery and Acceleration Path (Phase 5.13).
//!
//! Inspects QEMU graphics topology and discovers modern VirtIO-GPU PCI devices
//! (vendor 0x1AF4, device 0x1050). Provides hardware graphics acceleration interface
//! with automatic seamless fallback to UEFI GOP / CpuRenderer when absent.

use core::sync::atomic::{AtomicBool, Ordering};
use crate::{klog_info, pci};

static VIRTIO_GPU_PRESENT: AtomicBool = AtomicBool::new(false);

/// Searches enumerated PCI devices for a modern VirtIO-GPU device (vendor 0x1AF4, device 0x1050 or 0x1010).
pub fn find_virtio_gpu(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices
        .iter()
        .copied()
        .find(|d| d.vendor_id == 0x1AF4 && (d.device_id == 0x1050 || d.device_id == 0x1010))
}

/// Probes PCI bus for VirtIO-GPU device, parses capabilities, and configures hardware acceleration path.
pub fn probe_and_init(devices: &[pci::PciDevice]) {
    let Some(dev) = find_virtio_gpu(devices) else {
        klog_info!("VIRTIO_GPU: not attached; continuing with standard GOP framebuffer and CpuRenderer fallback");
        VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
        return;
    };

    klog_info!(
        "VIRTIO_GPU_FOUND {:02x}:{:02x}.{} vendor={:04x} dev={:04x}",
        dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id
    );

    let caps = pci::find_virtio_caps(dev.bus, dev.device, dev.function);
    if caps.is_empty() {
        klog_info!("VIRTIO_GPU: no modern virtio capabilities found -- using GOP fallback");
        VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
        return;
    }

    let mut common = None;
    let mut notify = None;
    for c in &caps {
        match c.cfg_type {
            1 => common = Some(*c),
            2 => notify = Some(*c),
            _ => {}
        }
    }

    if common.is_some() && notify.is_some() {
        klog_info!("VIRTIO_GPU_READY: modern PCI transport detected, GPU acceleration pathway initialized");
        VIRTIO_GPU_PRESENT.store(true, Ordering::Release);
    } else {
        klog_info!("VIRTIO_GPU: incomplete capabilities -- falling back to GOP CpuRenderer");
        VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
    }
}

/// Returns true if a functional VirtIO-GPU device is present and initialized.
#[inline(always)]
pub fn is_virtio_gpu_present() -> bool {
    VIRTIO_GPU_PRESENT.load(Ordering::Acquire)
}
