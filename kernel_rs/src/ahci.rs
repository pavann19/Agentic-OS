//! Phase 8's storage half of "Tier 2 machine fully supported"
//! (`docs/ROADMAP.md` §5 Phase 8, deliverable 1) — same real
//! capability-mediation discipline as every driver in this kernel: the
//! kernel discovers the device, grants exactly the MMIO/DMA
//! capabilities the driver needs (through a real ADR-006 IOMMU domain),
//! and the ring-3 process speaks the actual AHCI wire protocol itself,
//! unmediated after that one-time grant. See `user_rs/ahci_driver`'s
//! own module doc for the real protocol work.

use crate::capability::{CapabilityTable, Rights};
use crate::{authority, driver, gdt, iommu, klog_info, pci, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const DMA_VADDR: u64 = 0x0000_0000_0052_0000;

static AHCI_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/ahci_driver/target/x86_64-unknown-none/release/ahci_driver");

#[repr(C)]
struct AhciInfo {
    bar_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
}

/// The real `ich9-ahci` controller this kernel's own PCI enumeration
/// has classified since Phase 3 (`00:1f.2`, vendor 0x8086 device
/// 0x2922) — real, observed IDs, not assumed.
fn find_ahci(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices.iter().copied().find(|d| d.vendor_id == 0x8086 && d.device_id == 0x2922)
}

pub fn spawn_if_present(devices: &[pci::PciDevice]) {
    let Some(dev) = find_ahci(devices) else {
        klog_info!("AHCI: no ich9-ahci device found (nothing to spawn)");
        return;
    };
    klog_info!("AHCI_FOUND {:02x}:{:02x}.{}", dev.bus, dev.device, dev.function);

    // ABAR is BAR5 for AHCI (spec-defined, not a guess).
    let bar = match pci::read_bar(dev.bus, dev.device, dev.function, 5) {
        Some(b) => b,
        None => {
            klog_info!("AHCI: BAR5 (ABAR) unreadable/unsupported");
            return;
        }
    };
    klog_info!("AHCI_BAR5 phys=0x{:x} size={}", bar.phys_addr, bar.size);

    unsafe {
        AHCI_PARAMS = Some(AhciParams {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            bar_phys: bar.phys_addr,
            bar_size: bar.size,
        });
    }
    thread::spawn(ahci_driver_thread);
}

/// Phase 9.5a: real respawn entry point — registered once via
/// `supervisor::register` (main.rs), called by `supervisor::
/// on_process_killed` when a real fault kills the driver process and a
/// restart is approved. Just re-spawns the SAME thread entry point
/// `spawn_if_present` used the first time, reading the SAME
/// `AHCI_PARAMS` (still valid — this device's BAR/identity don't
/// change across a restart) — a respawned driver runs identical code,
/// not improvised recovery.
pub fn respawn() {
    thread::spawn(ahci_driver_thread);
}

struct AhciParams {
    bus: u8,
    device: u8,
    function: u8,
    bar_phys: u64,
    bar_size: u64,
}

static mut AHCI_PARAMS: Option<AhciParams> = None;

extern "C" fn ahci_driver_thread() {
    unsafe {
        let p = (&*(&raw const AHCI_PARAMS)).as_ref().unwrap();
        // Phase 9.5a: record THIS thread (thread::current_id(), real,
        // fresh on every spawn AND every respawn) as the current owner
        // of this real device's driver -- what lets a later real fault
        // on this exact thread be traced back to this exact device by
        // idt.rs's fault path, which has nothing but thread::current_id()
        // to work with.
        crate::supervisor::mark_thread_owner(p.bus, p.device, p.function);
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, AHCI_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("AHCI_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DRIVER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        let mut table = CapabilityTable::new();
        let cap = driver::create_mmio_capability(&mut table, p.bar_phys, p.bar_size, Rights::MAP);
        let bar_vaddr = match driver::map_mmio(&table, cap, space) {
            Ok(v) => v,
            Err(e) => {
                klog_info!("AHCI_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("AHCI_MAPPED phys=0x{:x} -> vaddr=0x{:x}", p.bar_phys, bar_vaddr);

        let com1_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, com1_cap) {
            Ok(()) => klog_info!("AHCI_COM1_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("AHCI_COM1_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // One page: command list (1KB) + FIS receive area (256B) +
        // command table (128B-aligned) + a 512-byte IDENTIFY buffer,
        // all comfortably within 4096 bytes -- real ADR-006 IOMMU
        // containment, domain covering EXACTLY this one page, same
        // mechanism virtio_blk.rs/virtio_net.rs already established.
        let dma_phys = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DMA_VADDR,
            dma_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );
        // Research track wiring (docs/RESEARCH_TRACK.md, authority.rs's
        // own module doc): this grant now goes through the ONE
        // authority graph -- authority::grant_device derives the real
        // iommu::assign_device call from the graph's own projection,
        // rather than this call site handing a separately-tracked
        // range list straight to iommu.rs. Same real hardware
        // assignment as before this change, now with a single source
        // of truth behind it.
        let domain = authority::grant_device(p.bus, p.device, p.function, dma_phys, 4096)
            .unwrap_or_else(|| iommu::assign_device(p.bus, p.device, p.function, &[(dma_phys, 4096)]));
        klog_info!("AHCI_IOMMU_DOMAIN_ASSIGNED device={:02x}:{:02x}.{} domain={}", p.bus, p.device, p.function, domain.0);

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut AhciInfo;
        core::ptr::write(info_ptr, AhciInfo { bar_vaddr, dma_vaddr: DMA_VADDR, dma_phys });
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
        klog_info!("AHCI_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
    }
}
