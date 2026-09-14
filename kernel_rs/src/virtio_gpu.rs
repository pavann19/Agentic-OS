//! VirtIO-GPU Graphics Device Discovery and Acceleration Path (Phase 12, Deliverable 2).
//!
//! Inspects QEMU graphics topology and configures modern VirtIO-GPU PCI devices
//! (vendor 0x1AF4, device 0x1050 or 0x1010). Implements modern VirtIO-PCI transport,
//! split-ring control virtqueue, and wire-format 2D commands (Resource Create 2D,
//! Attach Backing, Set Scanout, Transfer to Host 2D, and Resource Flush).
//! Provides seamless fallback to standard UEFI GOP / CpuRenderer when absent.

use core::sync::atomic::{AtomicBool, Ordering};
use crate::{klog_info, pci, pmm, vmm};

static VIRTIO_GPU_PRESENT: AtomicBool = AtomicBool::new(false);

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

#[allow(dead_code)]
const REG_DEVICE_FEATURE_SELECT: u64 = 0x00;
#[allow(dead_code)]
const REG_DEVICE_FEATURE: u64 = 0x04;
const REG_DRIVER_FEATURE_SELECT: u64 = 0x08;
const REG_DRIVER_FEATURE: u64 = 0x0C;
const REG_DEVICE_STATUS: u64 = 0x14;
const REG_QUEUE_SELECT: u64 = 0x16;
const REG_QUEUE_SIZE: u64 = 0x18;
const REG_QUEUE_ENABLE: u64 = 0x1C;
const REG_QUEUE_NOTIFY_OFF: u64 = 0x1E;
const REG_QUEUE_DESC: u64 = 0x20;
const REG_QUEUE_DRIVER: u64 = 0x28; // avail ring
const REG_QUEUE_DEVICE: u64 = 0x30; // used ring

pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_2D: u32 = 0x0101;
pub const VIRTIO_GPU_CMD_SET_SCANOUT: u32 = 0x0103;
pub const VIRTIO_GPU_CMD_RESOURCE_FLUSH: u32 = 0x0104;
pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D: u32 = 0x0105;
pub const VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING: u32 = 0x0106;

pub const VIRTIO_GPU_RESP_OK_NODATA: u32 = 0x1100;
pub const VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM: u32 = 1;

const QUEUE_SIZE: u16 = 16;
const DESC_OFF: u64 = 0x000;
const AVAIL_OFF: u64 = 0x100;
const USED_OFF: u64 = 0x200;
const REQ_OFF: u64 = 0x300;
const RESP_OFF: u64 = 0x500;
const BACKING_OFF: u64 = 0x800;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct VirtioGpuCtrlHdr {
    pub type_: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub ctx_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuResourceCreate2d {
    pub hdr: VirtioGpuCtrlHdr,
    pub resource_id: u32,
    pub format: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuMemEntry {
    pub addr: u64,
    pub length: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuResourceAttachBacking {
    pub hdr: VirtioGpuCtrlHdr,
    pub resource_id: u32,
    pub nr_entries: u32,
    pub entry: VirtioGpuMemEntry,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuSetScanout {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub scanout_id: u32,
    pub resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuTransferToHost2d {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub offset: u64,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuResourceFlush {
    pub hdr: VirtioGpuCtrlHdr,
    pub r: VirtioGpuRect,
    pub resource_id: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtioGpuRespOkNodata {
    pub hdr: VirtioGpuCtrlHdr,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

struct GpuState {
    #[allow(dead_code)]
    common_base_phys: u64,
    notify_addr_phys: u64,
    dma_phys: u64,
    avail_idx: u16,
    last_used_idx: u16,
}

static mut GPU_STATE: Option<GpuState> = None;

#[inline(always)]
unsafe fn mmio_read8(base_phys: u64, offset: u64) -> u8 {
    let vaddr = vmm::map_mmio_page(base_phys + offset);
    core::ptr::read_volatile(vaddr as *const u8)
}

#[inline(always)]
unsafe fn mmio_read16(base_phys: u64, offset: u64) -> u16 {
    let vaddr = vmm::map_mmio_page(base_phys + offset);
    core::ptr::read_volatile(vaddr as *const u16)
}

#[inline(always)]
unsafe fn mmio_write8(base_phys: u64, offset: u64, val: u8) {
    let vaddr = vmm::map_mmio_page(base_phys + offset);
    core::ptr::write_volatile(vaddr as *mut u8, val);
}

#[inline(always)]
unsafe fn mmio_write16(base_phys: u64, offset: u64, val: u16) {
    let vaddr = vmm::map_mmio_page(base_phys + offset);
    core::ptr::write_volatile(vaddr as *mut u16, val);
}

#[inline(always)]
unsafe fn mmio_write32(base_phys: u64, offset: u64, val: u32) {
    let vaddr = vmm::map_mmio_page(base_phys + offset);
    core::ptr::write_volatile(vaddr as *mut u32, val);
}

#[inline(always)]
unsafe fn mmio_write64(base_phys: u64, offset: u64, val: u64) {
    let vaddr = vmm::map_mmio_page(base_phys + offset);
    core::ptr::write_volatile(vaddr as *mut u64, val);
}

/// Searches enumerated PCI devices for a modern VirtIO-GPU device (vendor 0x1AF4, device 0x1050 or 0x1010).
pub fn find_virtio_gpu(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices
        .iter()
        .copied()
        .find(|d| d.vendor_id == 0x1AF4 && (d.device_id == 0x1050 || d.device_id == 0x1010))
}

unsafe fn send_cmd(
    state: &mut GpuState,
    req_len: u32,
    resp_len: u32,
) -> u32 {
    let dma_virt = pmm::p2v_pub(state.dma_phys);
    let desc_ptr = (dma_virt.add(DESC_OFF as usize)) as *mut VirtqDesc;

    // Desc 0: request buffer
    core::ptr::write_volatile(
        desc_ptr.add(0),
        VirtqDesc {
            addr: state.dma_phys + REQ_OFF,
            len: req_len,
            flags: 1, // VIRTQ_DESC_F_NEXT
            next: 1,
        },
    );

    // Desc 1: response buffer
    core::ptr::write_volatile(
        desc_ptr.add(1),
        VirtqDesc {
            addr: state.dma_phys + RESP_OFF,
            len: resp_len,
            flags: 2, // VIRTQ_DESC_F_WRITE
            next: 0,
        },
    );

    // Update avail ring
    let avail_ptr = dma_virt.add(AVAIL_OFF as usize);
    let ring_elem_ptr = (avail_ptr.add(4 + (state.avail_idx % QUEUE_SIZE) as usize * 2)) as *mut u16;
    core::ptr::write_volatile(ring_elem_ptr, 0);

    state.avail_idx = state.avail_idx.wrapping_add(1);
    let avail_idx_ptr = (avail_ptr.add(2)) as *mut u16;
    core::ptr::write_volatile(avail_idx_ptr, state.avail_idx);

    core::sync::atomic::fence(Ordering::SeqCst);

    // Notify queue 0
    mmio_write16(state.notify_addr_phys, 0, 0);

    // Poll used ring for completion
    let used_ptr = dma_virt.add(USED_OFF as usize);
    let used_idx_ptr = (used_ptr.add(2)) as *const u16;
    let mut spins = 0u64;
    while core::ptr::read_volatile(used_idx_ptr) == state.last_used_idx {
        spins += 1;
        if spins > 20_000_000 {
            klog_info!("VIRTIO_GPU: command timeout waiting for device completion");
            return 0;
        }
        core::hint::spin_loop();
    }
    state.last_used_idx = core::ptr::read_volatile(used_idx_ptr);

    // Read response header type
    let resp_hdr = (dma_virt.add(RESP_OFF as usize)) as *const VirtioGpuCtrlHdr;
    core::ptr::read_volatile(resp_hdr).type_
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

    // Enable PCI Memory Space & Bus Master
    let cmd = pci::read_config_u16(dev.bus, dev.device, dev.function, 0x04);
    unsafe {
        pci::write_config_u32(dev.bus, dev.device, dev.function, 0x04, (cmd as u32) | 0x0006);
    }

    for i in 0..6u8 {
        let raw = pci::read_config_u32(dev.bus, dev.device, dev.function, 0x10 + i * 4);
        klog_info!("VIRTIO_GPU_BAR{} raw=0x{:08x}", i, raw);
    }

    let caps = pci::find_virtio_caps(dev.bus, dev.device, dev.function);
    for c in &caps {
        klog_info!("VIRTIO_GPU_CAP type={} bar={} off=0x{:x} len=0x{:x} mult={}", c.cfg_type, c.bar, c.offset, c.length, c.notify_off_multiplier);
    }
    if caps.is_empty() {
        klog_info!("VIRTIO_GPU: no modern virtio capabilities found -- using GOP fallback");
        VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
        return;
    }

    let mut common_cap = None;
    let mut notify_cap = None;
    for c in &caps {
        match c.cfg_type {
            1 => common_cap = Some(*c),
            2 => notify_cap = Some(*c),
            _ => {}
        }
    }

    let (Some(common), Some(notify)) = (common_cap, notify_cap) else {
        klog_info!("VIRTIO_GPU: incomplete capabilities -- falling back to GOP CpuRenderer");
        VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
        return;
    };

    let Some(bar) = pci::read_bar(dev.bus, dev.device, dev.function, common.bar) else {
        klog_info!("VIRTIO_GPU: common BAR{} unreadable -- falling back to GOP", common.bar);
        VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
        return;
    };

    let common_base = bar.phys_addr + (common.offset as u64);

    unsafe {
        // Reset device
        mmio_write8(common_base, REG_DEVICE_STATUS, 0);
        let mut reset_spins = 0u32;
        while mmio_read8(common_base, REG_DEVICE_STATUS) != 0 {
            reset_spins += 1;
            if reset_spins > 100_000 { break; }
            core::hint::spin_loop();
        }

        // Acknowledge & Driver
        mmio_write8(common_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        // Feature negotiation
        mmio_write32(common_base, REG_DRIVER_FEATURE_SELECT, 1);
        mmio_write32(common_base, REG_DRIVER_FEATURE, 1); // VIRTIO_F_VERSION_1

        mmio_write8(common_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        if mmio_read8(common_base, REG_DEVICE_STATUS) & STATUS_FEATURES_OK == 0 {
            klog_info!("VIRTIO_GPU: FEATURES_OK rejected -- falling back to GOP");
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }

        // Queue 0 (controlq) setup
        mmio_write16(common_base, REG_QUEUE_SELECT, 0);
        let max_size = mmio_read16(common_base, REG_QUEUE_SIZE);
        if max_size == 0 {
            klog_info!("VIRTIO_GPU: controlq size is 0 -- falling back to GOP");
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }

        let dma_phys = pmm::alloc_page();
        if dma_phys == 0 {
            klog_info!("VIRTIO_GPU: failed to allocate DMA page -- falling back to GOP");
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }

        let dma_virt = pmm::p2v_pub(dma_phys);
        core::ptr::write_bytes(dma_virt, 0, 4096);

        mmio_write16(common_base, REG_QUEUE_SIZE, QUEUE_SIZE);
        mmio_write64(common_base, REG_QUEUE_DESC, dma_phys + DESC_OFF);
        mmio_write64(common_base, REG_QUEUE_DRIVER, dma_phys + AVAIL_OFF);
        mmio_write64(common_base, REG_QUEUE_DEVICE, dma_phys + USED_OFF);

        let queue_notify_off = mmio_read16(common_base, REG_QUEUE_NOTIFY_OFF);
        let notify_addr = bar.phys_addr + (notify.offset as u64) + (queue_notify_off as u64) * (notify.notify_off_multiplier as u64);

        mmio_write16(common_base, REG_QUEUE_ENABLE, 1);

        // Driver OK
        mmio_write8(common_base, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);

        klog_info!("VIRTIO_GPU_READY: modern PCI transport detected, controlq enabled");

        let mut state = GpuState {
            common_base_phys: common_base,
            notify_addr_phys: notify_addr,
            dma_phys,
            avail_idx: 0,
            last_used_idx: 0,
        };

        // Self-check: Execute 2D hardware commands
        // 1. RESOURCE_CREATE_2D (res_id=1, 64x64, B8G8R8A8_UNORM)
        let req_create = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_CREATE_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: 1,
            format: VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM,
            width: 64,
            height: 64,
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuResourceCreate2d, req_create);
        let resp1 = send_cmd(&mut state, core::mem::size_of::<VirtioGpuResourceCreate2d>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);
        if resp1 != VIRTIO_GPU_RESP_OK_NODATA {
            klog_info!("VIRTIO_GPU: RESOURCE_CREATE_2D failed code=0x{:x}", resp1);
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }
        klog_info!("VIRTIO_GPU_RESOURCE_CREATE_PASS res_id=1");

        // 2. RESOURCE_ATTACH_BACKING
        let req_attach = VirtioGpuResourceAttachBacking {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_ATTACH_BACKING,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            resource_id: 1,
            nr_entries: 1,
            entry: VirtioGpuMemEntry {
                addr: dma_phys + BACKING_OFF,
                length: 2048,
                padding: 0,
            },
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuResourceAttachBacking, req_attach);
        let resp2 = send_cmd(&mut state, core::mem::size_of::<VirtioGpuResourceAttachBacking>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);
        if resp2 != VIRTIO_GPU_RESP_OK_NODATA {
            klog_info!("VIRTIO_GPU: RESOURCE_ATTACH_BACKING failed code=0x{:x}", resp2);
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }
        klog_info!("VIRTIO_GPU_ATTACH_BACKING_PASS");

        // 3. SET_SCANOUT
        let req_scanout = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_SET_SCANOUT,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x: 0, y: 0, width: 64, height: 64 },
            scanout_id: 0,
            resource_id: 1,
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuSetScanout, req_scanout);
        let resp3 = send_cmd(&mut state, core::mem::size_of::<VirtioGpuSetScanout>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);
        if resp3 != VIRTIO_GPU_RESP_OK_NODATA {
            klog_info!("VIRTIO_GPU: SET_SCANOUT failed code=0x{:x}", resp3);
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }
        klog_info!("VIRTIO_GPU_SET_SCANOUT_PASS");

        // 4. TRANSFER_TO_HOST_2D
        let req_transfer = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x: 0, y: 0, width: 64, height: 64 },
            offset: 0,
            resource_id: 1,
            padding: 0,
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuTransferToHost2d, req_transfer);
        let resp4 = send_cmd(&mut state, core::mem::size_of::<VirtioGpuTransferToHost2d>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);
        if resp4 != VIRTIO_GPU_RESP_OK_NODATA {
            klog_info!("VIRTIO_GPU: TRANSFER_TO_HOST_2D failed code=0x{:x}", resp4);
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }
        klog_info!("VIRTIO_GPU_TRANSFER_PASS");

        // 5. RESOURCE_FLUSH
        let req_flush = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x: 0, y: 0, width: 64, height: 64 },
            resource_id: 1,
            padding: 0,
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuResourceFlush, req_flush);
        let resp5 = send_cmd(&mut state, core::mem::size_of::<VirtioGpuResourceFlush>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);
        if resp5 != VIRTIO_GPU_RESP_OK_NODATA {
            klog_info!("VIRTIO_GPU: RESOURCE_FLUSH failed code=0x{:x}", resp5);
            VIRTIO_GPU_PRESENT.store(false, Ordering::Release);
            return;
        }
        klog_info!("VIRTIO_GPU_FLUSH_PASS");

        GPU_STATE = Some(state);
        VIRTIO_GPU_PRESENT.store(true, Ordering::Release);
        klog_info!("VIRTIO_GPU_2D_ACCEL_ACTIVE: hardware 2D pipeline operational");
    }
}

/// Returns true if a functional VirtIO-GPU device is present and initialized.
#[inline(always)]
pub fn is_virtio_gpu_present() -> bool {
    VIRTIO_GPU_PRESENT.load(Ordering::Acquire)
}

/// Flushes dirty rectangle to GPU display using hardware 2D commands.
pub fn flush_rect(x: u32, y: u32, width: u32, height: u32) -> bool {
    if !is_virtio_gpu_present() {
        return false;
    }
    unsafe {
        #[allow(static_mut_refs)]
        let Some(ref mut state) = GPU_STATE else {
            return false;
        };
        let dma_virt = pmm::p2v_pub(state.dma_phys);

        // Transfer to host 2D
        let req_transfer = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x, y, width, height },
            offset: 0,
            resource_id: 1,
            padding: 0,
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuTransferToHost2d, req_transfer);
        let _ = send_cmd(state, core::mem::size_of::<VirtioGpuTransferToHost2d>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);

        // Resource flush
        let req_flush = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                type_: VIRTIO_GPU_CMD_RESOURCE_FLUSH,
                flags: 0,
                fence_id: 0,
                ctx_id: 0,
                padding: 0,
            },
            r: VirtioGpuRect { x, y, width, height },
            resource_id: 1,
            padding: 0,
        };
        core::ptr::write((dma_virt.add(REQ_OFF as usize)) as *mut VirtioGpuResourceFlush, req_flush);
        let resp = send_cmd(state, core::mem::size_of::<VirtioGpuResourceFlush>() as u32, core::mem::size_of::<VirtioGpuRespOkNodata>() as u32);
        resp == VIRTIO_GPU_RESP_OK_NODATA
    }
}
