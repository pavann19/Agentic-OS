//! Phase 8's other storage half of "Tier 2 machine fully supported"
//! (`docs/ROADMAP.md` §5 Phase 8, deliverable 1) — same real
//! capability-mediation discipline as every driver in this kernel: the
//! kernel discovers the device, grants exactly the MMIO/DMA
//! capabilities the driver needs (through a real ADR-006 IOMMU
//! domain), and the ring-3 process speaks the actual NVMe wire
//! protocol itself, unmediated after that one-time grant. See
//! `user_rs/nvme_driver`'s own module doc for the real protocol work.
//!
//! Real, spec-driven layout choice, not incidental: NVMe requires the
//! admin submission queue AND the admin completion queue to each be
//! independently "Memory Page Aligned" (NVMe spec 3.1.9/3.1.10) — with
//! `CC.MPS=0` (this driver's own choice, matching every other page-size
//! assumption in this kernel), that means each needs its OWN dedicated
//! 4KB-aligned physical page, not two structures packed into one page
//! the way `ahci_driver`'s command list/FIS/command table share one.
//! This is why this driver gets THREE physical pages (ASQ, ACQ, and a
//! separate Identify-data buffer), not one.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, iommu, klog_info, pci, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const ASQ_VADDR: u64 = 0x0000_0000_0052_0000;
const ACQ_VADDR: u64 = 0x0000_0000_0053_0000;
const DATA_VADDR: u64 = 0x0000_0000_0054_0000;

static NVME_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/nvme_driver/target/x86_64-unknown-none/release/nvme_driver");

#[repr(C)]
struct NvmeInfo {
    bar_vaddr: u64,
    asq_vaddr: u64,
    asq_phys: u64,
    acq_vaddr: u64,
    acq_phys: u64,
    data_vaddr: u64,
    data_phys: u64,
}

/// Real, modern NVMe controller class match (PCI class 0x01, subclass
/// 0x08, prog-if 0x02 = "NVM Express") — matched by CLASS, not a fixed
/// vendor/device ID, since QEMU's own `-device nvme` (and real NVMe
/// SSDs from different vendors) can legitimately report different
/// vendor/device IDs while all being real NVMe controllers; the class
/// code is what the spec actually guarantees.
fn find_nvme(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices
        .iter()
        .copied()
        .find(|d| d.class == 0x01 && d.subclass == 0x08 && d.prog_if == 0x02)
}

pub fn spawn_if_present(devices: &[pci::PciDevice]) {
    let Some(dev) = find_nvme(devices) else {
        klog_info!("NVME: no NVMe controller found (nothing to spawn)");
        return;
    };
    klog_info!("NVME_FOUND {:02x}:{:02x}.{}", dev.bus, dev.device, dev.function);

    // BAR0 (64-bit) is the NVMe controller register set (spec 2.1.1).
    let bar = match pci::read_bar(dev.bus, dev.device, dev.function, 0) {
        Some(b) => b,
        None => {
            klog_info!("NVME: BAR0 unreadable/unsupported");
            return;
        }
    };
    klog_info!("NVME_BAR0 phys=0x{:x} size={}", bar.phys_addr, bar.size);

    unsafe {
        NVME_PARAMS = Some(NvmeParams {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            bar_phys: bar.phys_addr,
            bar_size: bar.size,
        });
    }
    thread::spawn(nvme_driver_thread);
}

struct NvmeParams {
    bus: u8,
    device: u8,
    function: u8,
    bar_phys: u64,
    bar_size: u64,
}

static mut NVME_PARAMS: Option<NvmeParams> = None;

extern "C" fn nvme_driver_thread() {
    unsafe {
        let p = (&*(&raw const NVME_PARAMS)).as_ref().unwrap();
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, NVME_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("NVME_ELF_LOAD_FAILED {:?}", e);
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
                klog_info!("NVME_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("NVME_MAPPED phys=0x{:x} -> vaddr=0x{:x}", p.bar_phys, bar_vaddr);

        let com1_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, com1_cap) {
            Ok(()) => klog_info!("NVME_COM1_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("NVME_COM1_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // Three REAL, independently page-aligned physical pages -- see
        // this module's own doc for why NVMe needs that (unlike
        // ahci_driver's single shared page). Real ADR-006 IOMMU
        // containment: one domain covering exactly these three pages,
        // same mechanism every other driver in this kernel already
        // establishes.
        let asq_phys = pmm::alloc_page();
        let acq_phys = pmm::alloc_page();
        let data_phys = pmm::alloc_page();
        vmm::map_page_in(space, ASQ_VADDR, asq_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        vmm::map_page_in(space, ACQ_VADDR, acq_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        vmm::map_page_in(space, DATA_VADDR, data_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let domain = iommu::assign_device(
            p.bus,
            p.device,
            p.function,
            &[(asq_phys, 4096), (acq_phys, 4096), (data_phys, 4096)],
        );
        klog_info!("NVME_IOMMU_DOMAIN_ASSIGNED device={:02x}:{:02x}.{} domain={}", p.bus, p.device, p.function, domain.0);

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut NvmeInfo;
        core::ptr::write(
            info_ptr,
            NvmeInfo {
                bar_vaddr,
                asq_vaddr: ASQ_VADDR,
                asq_phys,
                acq_vaddr: ACQ_VADDR,
                acq_phys,
                data_vaddr: DATA_VADDR,
                data_phys,
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
        klog_info!("NVME_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
    }
}
