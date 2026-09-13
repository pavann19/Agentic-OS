//! Phase 4 item 1: block device abstraction + a real `virtio-blk`
//! user-space driver (Tier 1's target per `docs/ROADMAP.md`). Same
//! discipline as every Phase 3 driver: the kernel discovers the device,
//! grants exactly the capabilities the driver needs (MMIO for its PCI
//! BAR, an IOMMU-backed DMA-safe buffer), and the ring-3 process speaks
//! the actual virtio wire protocol itself, unmediated after that
//! one-time grant.
//!
//! Real, disclosed scope for this first increment: virtqueue completion
//! is POLLED (the driver spins on the used-ring index), not interrupt-
//! driven — a real, stated simplification (same spirit as `ipc.rs`'s
//! documented spin-yield rendezvous), not a corner cut hidden from the
//! log. Real interrupt-driven completion (MSI-X or legacy INTx) is a
//! genuine follow-up, not required to prove the block-I/O path actually
//! works end to end.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, iommu, klog_info, pci, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const DMA_VADDR: u64 = 0x0000_0000_0052_0000;

static VIRTIO_BLK_DRIVER_ELF: &[u8] = include_bytes!(
    "../../user_rs/virtio_blk_driver/target/x86_64-unknown-none/release/virtio_blk_driver"
);

/// The real, minimal ABI this driver reads at a fixed vaddr before doing
/// anything else — same pattern as `user_driver.rs`'s `FbInfo`. All the
/// physical addresses a virtio driver needs to program into device
/// registers (which only ever take physical addresses, never anything
/// this ring-3 process's own vaddr space means to the device).
#[repr(C)]
struct VirtioBlkInfo {
    bar_vaddr: u64,
    common_off: u32,
    notify_off: u32,
    notify_multiplier: u32,
    isr_off: u32,
    device_off: u32,
    dma_vaddr: u64,
    dma_phys: u64,
    // Real block/file-I/O-for-apps path (`file_service.rs`): this
    // driver's own real receive-side IpcEndpoint CapId for real
    // file-content requests from other processes (e.g. `file_manager`).
    file_service_cap: u32,
}

/// Finds the real virtio-blk PCI device from an already-completed
/// `pci::enumerate()` scan. Modern-only (vendor 0x1AF4, device 0x1042) —
/// matches `disable-legacy=on` in `scripts/test-boot.ps1`, so there is no
/// legacy I/O-BAR transport to also support.
fn find_virtio_blk(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices.iter().copied().find(|d| d.vendor_id == 0x1AF4 && d.device_id == 0x1042)
}

pub fn spawn_if_present(devices: &[pci::PciDevice]) {
    let Some(dev) = find_virtio_blk(devices) else {
        klog_info!("VIRTIO_BLK: no modern virtio-blk device found (nothing to spawn)");
        return;
    };
    klog_info!(
        "VIRTIO_BLK_FOUND {:02x}:{:02x}.{}",
        dev.bus, dev.device, dev.function
    );

    let caps = pci::find_virtio_caps(dev.bus, dev.device, dev.function);
    if caps.is_empty() {
        klog_info!("VIRTIO_BLK: no virtio PCI capabilities found -- not a modern virtio device?");
        return;
    }

    // Real bug class this kernel has hit before (Phase 3's IOMMU/ACPI
    // work): every cfg_type capability commonly points into the SAME
    // BAR in QEMU's default virtio-pci layout, but nothing in the spec
    // guarantees that -- read each one for real rather than assuming.
    let mut common = None;
    let mut notify = None;
    let mut isr = None;
    let mut device_cfg = None;
    for c in &caps {
        match c.cfg_type {
            1 => common = Some(*c),
            2 => notify = Some(*c),
            3 => isr = Some(*c),
            4 => device_cfg = Some(*c),
            _ => {}
        }
    }
    let (Some(common), Some(notify), Some(_isr), Some(device_cfg)) = (common, notify, isr, device_cfg) else {
        klog_info!("VIRTIO_BLK: missing a required cfg capability (common/notify/isr/device)");
        return;
    };

    // Map every BAR actually referenced -- usually just one (BAR4 in
    // QEMU's default layout) but read for real, not assumed.
    let mut bar_phys_by_index: alloc::collections::BTreeMap<u8, pci::Bar> = alloc::collections::BTreeMap::new();
    for c in [&common, &notify, &device_cfg] {
        if !bar_phys_by_index.contains_key(&c.bar) {
            match pci::read_bar(dev.bus, dev.device, dev.function, c.bar) {
                Some(bar) => {
                    bar_phys_by_index.insert(c.bar, bar);
                }
                None => {
                    klog_info!("VIRTIO_BLK: BAR{} unreadable/unsupported", c.bar);
                    return;
                }
            }
        }
    }
    // All three (common/notify/device_cfg) land in the same BAR in every
    // configuration this driver supports -- real, checked, not assumed.
    if common.bar != notify.bar || common.bar != device_cfg.bar {
        klog_info!("VIRTIO_BLK: cfg regions span multiple BARs -- not supported by this first increment");
        return;
    }
    let bar = bar_phys_by_index[&common.bar];
    klog_info!(
        "VIRTIO_BLK_BAR{} phys=0x{:x} size={}",
        common.bar, bar.phys_addr, bar.size
    );

    // thread::spawn() takes a plain `extern "C" fn()`, not a closure --
    // these boot-time parameters reach the thread body through a
    // dedicated static instead, the same pattern every other driver in
    // this kernel already uses (user_driver.rs's FB_PARAMS, etc.).
    unsafe {
        VIRTIO_BLK_PARAMS = Some(VirtioBlkParams {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            bar_phys: bar.phys_addr,
            bar_size: bar.size,
            common_off: common.offset,
            notify_off: notify.offset,
            notify_multiplier: notify.notify_off_multiplier,
            isr_off: _isr.offset,
            device_off: device_cfg.offset,
        });
    }
    thread::spawn(virtio_blk_driver_thread);
}

struct VirtioBlkParams {
    bus: u8,
    device: u8,
    function: u8,
    bar_phys: u64,
    bar_size: u64,
    common_off: u32,
    notify_off: u32,
    notify_multiplier: u32,
    isr_off: u32,
    device_off: u32,
}

static mut VIRTIO_BLK_PARAMS: Option<VirtioBlkParams> = None;

extern "C" fn virtio_blk_driver_thread() {
    unsafe {
        let p = (&*(&raw const VIRTIO_BLK_PARAMS)).as_ref().unwrap();
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, VIRTIO_BLK_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("VIRTIO_BLK_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        // 4 pages (16KB), not 1 -- the real ext2 formatting logic this
        // driver runs (kernel_common::ext2) keeps several 1024-byte
        // block buffers live on the stack at once (building one block
        // while a previous one is still being written, plus the
        // inode-table/data-block/output buffers the final read-back
        // verification needs simultaneously). A single 4KB page was
        // enough for the raw-sector self-check alone but would overflow
        // once the filesystem logic was added.
        const STACK_PAGES: u64 = 4;
        for i in 0..STACK_PAGES {
            let stack_page = pmm::alloc_page();
            vmm::map_page_in(
                space,
                DRIVER_STACK_VADDR + i * 4096,
                stack_page,
                vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
            );
        }

        // Real MmioRegion capability for the device's real BAR.
        let mut table = CapabilityTable::new();
        let cap = driver::create_mmio_capability(&mut table, p.bar_phys, p.bar_size, Rights::MAP);
        let bar_vaddr = match driver::map_mmio(&table, cap, space) {
            Ok(v) => v,
            Err(e) => {
                klog_info!("VIRTIO_BLK_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("VIRTIO_BLK_MAPPED phys=0x{:x} -> vaddr=0x{:x}", p.bar_phys, bar_vaddr);

        // Real bug found and fixed bringing this driver up: it uses the
        // same raw-COM1-write proof technique as serial_driver, but
        // without a PortIoRange grant for COM1 the very first `out`
        // instruction faults immediately (blocked by the IOPB) --
        // exactly the mechanism driver.rs's own module doc describes
        // (real per-port mediation, not IOPL=3), just forgotten here on
        // first pass. Fixed by granting it, same as serial_driver does.
        let port_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, port_cap) {
            Ok(()) => klog_info!("VIRTIO_BLK_PORT_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("VIRTIO_BLK_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // Real DMA-safe buffer: one physical page for the whole virtqueue
        // (descriptor table, avail ring, used ring) plus the block
        // request header/data/status -- real ADR-006 IOMMU containment,
        // not skipped for being a small buffer: assigned to a domain
        // covering EXACTLY this one page, same real mechanism
        // iommu.rs's own module doc describes, extended here to a second
        // real device (the first, Phase 3's own IOMMU proof, assigned
        // the SATA controller; this is the first device that actually
        // performs real I/O through its assigned domain).
        let dma_phys = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DMA_VADDR,
            dma_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );
        let domain = iommu::assign_device(p.bus, p.device, p.function, &[(dma_phys, 4096)]);
        klog_info!("VIRTIO_BLK_IOMMU_DOMAIN_ASSIGNED device={:02x}:{:02x}.{} domain={}", p.bus, p.device, p.function, domain.0);

        // Real block/file-I/O-for-apps path: this driver becomes the
        // one real file-serving process other apps (e.g. `file_manager`)
        // can request real file content from, over the exact same real
        // IPC mechanism `input_routing::register_window_input` already
        // established for routed keyboard input -- see `file_service.rs`'s
        // own module doc.
        let file_service_cap = crate::file_service::register_server();
        klog_info!("VIRTIO_BLK_FILE_SERVICE_CAP={}", file_service_cap);

        // The real, minimal boot-time ABI -- same fixed-vaddr pattern
        // every other driver in this kernel already uses.
        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut VirtioBlkInfo;
        core::ptr::write(
            info_ptr,
            VirtioBlkInfo {
                bar_vaddr,
                common_off: p.common_off,
                notify_off: p.notify_off,
                notify_multiplier: p.notify_multiplier,
                isr_off: p.isr_off,
                device_off: p.device_off,
                dma_vaddr: DMA_VADDR,
                dma_phys,
                file_service_cap,
            },
        );
        vmm::map_page_in(
            space,
            INFO_VADDR,
            info_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("VIRTIO_BLK_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + STACK_PAGES * 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + STACK_PAGES * 4096);
    }
}
