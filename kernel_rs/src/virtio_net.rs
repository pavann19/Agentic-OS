//! Phase 6's first synthesis target (`docs/ROADMAP.md` §5 Phase 6,
//! deliverable 4): "`virtio-net` on Tier 1, with the in-tree driver
//! absent. Spec plus PCI configuration space as the only inputs." This
//! is that target — worked from the actual VIRTIO 1.0/1.1 network
//! device spec (§5.1), NOT copied from `virtio_blk.rs`'s wire protocol
//! (only the shared PCI/MMIO/capability-grant SCAFFOLDING is reused,
//! since that part is genuinely the same transport every modern virtio
//! device speaks — the device-specific parts below are new: two
//! virtqueues instead of one, `virtio_net_config` instead of a block
//! header, a `virtio_net_hdr` framing every packet, real Ethernet/ARP
//! frame construction).
//!
//! Same real capability-mediation discipline as every other driver in
//! this kernel: the kernel discovers the device, grants exactly the
//! MMIO/port/DMA capabilities the driver needs (through a real
//! `ADR-006` IOMMU domain, same as `virtio_blk.rs`), and the ring-3
//! process speaks the actual wire protocol itself, unmediated after
//! that one-time grant.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, iommu, klog_info, pci, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const DMA_VADDR: u64 = 0x0000_0000_0052_0000;

// Real induced-failure trial support (this crate's own Cargo feature
// `synthesis_induced_fault`, off by default): embeds the SAME crate's
// deliberately-buggy build (its own `induced_fault` feature -- a TX
// descriptor pointing outside its assigned IOMMU domain) instead of the
// normal one, so `scripts/test-synthesis-fault-demo.ps1` can boot a
// kernel that genuinely exercises Phase 6's induced-failure and IOMMU
// containment exit criteria, real hardware-level DMA blocking included,
// not simulated.
#[cfg(not(feature = "synthesis_induced_fault"))]
static VIRTIO_NET_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/virtio_net_driver/variants/virtio_net_driver_good");
#[cfg(feature = "synthesis_induced_fault")]
static VIRTIO_NET_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/virtio_net_driver/variants/virtio_net_driver_induced_fault");

#[repr(C)]
struct VirtioNetInfo {
    bar_vaddr: u64,
    common_off: u32,
    notify_off: u32,
    notify_multiplier: u32,
    isr_off: u32,
    device_off: u32,
    dma_vaddr: u64,
    dma_phys: u64,
}

/// Real, modern-only virtio-net device (vendor 0x1AF4, device 0x1041) —
/// matches `disable-legacy=on` in `scripts/test-synthesis.ps1`, the same
/// discipline `virtio_blk.rs` already uses.
fn find_virtio_net(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices.iter().copied().find(|d| d.vendor_id == 0x1AF4 && d.device_id == 0x1041)
}

pub fn spawn_if_present(devices: &[pci::PciDevice]) {
    let Some(dev) = find_virtio_net(devices) else {
        klog_info!("VIRTIO_NET: no modern virtio-net device found (nothing to spawn)");
        return;
    };
    klog_info!("VIRTIO_NET_FOUND {:02x}:{:02x}.{}", dev.bus, dev.device, dev.function);

    let caps = pci::find_virtio_caps(dev.bus, dev.device, dev.function);
    if caps.is_empty() {
        klog_info!("VIRTIO_NET: no virtio PCI capabilities found -- not a modern virtio device?");
        return;
    }

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
        klog_info!("VIRTIO_NET: missing a required cfg capability (common/notify/isr/device)");
        return;
    };

    let mut bar_phys_by_index: alloc::collections::BTreeMap<u8, pci::Bar> = alloc::collections::BTreeMap::new();
    for c in [&common, &notify, &device_cfg] {
        if !bar_phys_by_index.contains_key(&c.bar) {
            match pci::read_bar(dev.bus, dev.device, dev.function, c.bar) {
                Some(bar) => {
                    bar_phys_by_index.insert(c.bar, bar);
                }
                None => {
                    klog_info!("VIRTIO_NET: BAR{} unreadable/unsupported", c.bar);
                    return;
                }
            }
        }
    }
    if common.bar != notify.bar || common.bar != device_cfg.bar {
        klog_info!("VIRTIO_NET: cfg regions span multiple BARs -- not supported by this increment");
        return;
    }
    let bar = bar_phys_by_index[&common.bar];
    klog_info!("VIRTIO_NET_BAR{} phys=0x{:x} size={}", common.bar, bar.phys_addr, bar.size);

    unsafe {
        VIRTIO_NET_PARAMS = Some(VirtioNetParams {
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
    thread::spawn(virtio_net_driver_thread);
}

struct VirtioNetParams {
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

static mut VIRTIO_NET_PARAMS: Option<VirtioNetParams> = None;

extern "C" fn virtio_net_driver_thread() {
    unsafe {
        let p = (&*(&raw const VIRTIO_NET_PARAMS)).as_ref().unwrap();
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, VIRTIO_NET_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("VIRTIO_NET_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        const STACK_PAGES: u64 = 2;
        for i in 0..STACK_PAGES {
            let stack_page = pmm::alloc_page();
            vmm::map_page_in(
                space,
                DRIVER_STACK_VADDR + i * 4096,
                stack_page,
                vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
            );
        }

        let mut table = CapabilityTable::new();
        let cap = driver::create_mmio_capability(&mut table, p.bar_phys, p.bar_size, Rights::MAP);
        let bar_vaddr = match driver::map_mmio(&table, cap, space) {
            Ok(v) => v,
            Err(e) => {
                klog_info!("VIRTIO_NET_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("VIRTIO_NET_MAPPED phys=0x{:x} -> vaddr=0x{:x}", p.bar_phys, bar_vaddr);

        let port_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, port_cap) {
            Ok(()) => klog_info!("VIRTIO_NET_PORT_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("VIRTIO_NET_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // One page: two small virtqueues (RX+TX) plus TX/RX packet
        // buffers all comfortably fit (see user_rs/virtio_net_driver's
        // own offset layout doc) -- real ADR-006 IOMMU containment,
        // domain covering EXACTLY this one page, same mechanism
        // virtio_blk.rs already established for a second real device.
        let dma_phys = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DMA_VADDR,
            dma_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );
        let domain = iommu::assign_device(p.bus, p.device, p.function, &[(dma_phys, 4096)]);
        klog_info!("VIRTIO_NET_IOMMU_DOMAIN_ASSIGNED device={:02x}:{:02x}.{} domain={}", p.bus, p.device, p.function, domain.0);

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut VirtioNetInfo;
        core::ptr::write(
            info_ptr,
            VirtioNetInfo {
                bar_vaddr,
                common_off: p.common_off,
                notify_off: p.notify_off,
                notify_multiplier: p.notify_multiplier,
                isr_off: p.isr_off,
                device_off: p.device_off,
                dma_vaddr: DMA_VADDR,
                dma_phys,
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
        klog_info!("VIRTIO_NET_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + STACK_PAGES * 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + STACK_PAGES * 4096);
    }
}
