//! Phase 11 (`docs/ROADMAP.md` §5, deliverable 2): kernel-side spawn
//! code for the real xHCI (USB) driver — same real capability-
//! mediation discipline as every driver in this kernel: the kernel
//! discovers the device, grants exactly the MMIO capability the
//! driver needs (through a real ADR-006 IOMMU domain), and the ring-3
//! process (`user_rs/usb_xhci_driver`) speaks the actual xHCI wire
//! protocol itself, unmediated after that one-time grant. See that
//! crate's own module doc for the real protocol work.
//!
//! Real, disclosed difference from `ahci.rs`/`nvme.rs`: those find
//! their device by exact, observed vendor/device ID (QEMU's own
//! fixed `ich9-ahci`/`nvme` emulation). An xHCI controller's
//! vendor/device ID varies by emulator and real vendor (QEMU's
//! `qemu-xhci` reports Red Hat's own IDs, a real machine reports its
//! chipset vendor's) — the actually correct, spec-mandated way to
//! find ANY xHCI controller is PCI class code (0x0C, Serial Bus
//! Controller), subclass (0x03, USB), and prog-if (0x30, XHCI) — the
//! same real, portable identification a real OS driver uses, not a
//! QEMU-specific shortcut.

use crate::capability::{CapabilityTable, Rights};
use crate::{authority, driver, gdt, iommu, klog_info, pci, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const DMA_VADDR: u64 = 0x0000_0000_0052_0000;

static XHCI_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/usb_xhci_driver/target/x86_64-unknown-none/release/usb_xhci_driver");

#[repr(C)]
struct XhciInfo {
    bar_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
}

/// Real, spec-mandated identification (PCI class 0x0C / subclass 0x03
/// / prog-if 0x30) — see this module's own doc for why exact
/// vendor/device ID matching, correct for AHCI/NVMe, is wrong here.
fn find_xhci(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices.iter().copied().find(|d| d.class == 0x0C && d.subclass == 0x03 && d.prog_if == 0x30)
}

pub fn spawn_if_present(devices: &[pci::PciDevice]) {
    let Some(dev) = find_xhci(devices) else {
        klog_info!("XHCI: no xHCI (USB 3.x) host controller found (nothing to spawn)");
        return;
    };
    klog_info!("XHCI_FOUND {:02x}:{:02x}.{} vendor=0x{:04x} device=0x{:04x}", dev.bus, dev.device, dev.function, dev.vendor_id, dev.device_id);

    // BAR0 carries the xHCI MMIO register space (xHCI spec 5.1, PCI
    // Base Address Register — always BAR0 for an xHCI controller, not
    // a guess).
    let bar = match pci::read_bar(dev.bus, dev.device, dev.function, 0) {
        Some(b) => b,
        None => {
            klog_info!("XHCI: BAR0 unreadable/unsupported");
            return;
        }
    };
    klog_info!("XHCI_BAR0 phys=0x{:x} size={}", bar.phys_addr, bar.size);

    unsafe {
        XHCI_PARAMS = Some(XhciParams {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            bar_phys: bar.phys_addr,
            bar_size: bar.size,
        });
    }
    // Phase 9.5a reuse, same real registration every driver since AHCI
    // gets — a real fault on this driver is observed and restarted by
    // the same real supervisor mechanism, not a special case.
    crate::supervisor::register(dev.bus, dev.device, dev.function, respawn);
    thread::spawn(xhci_driver_thread);
}

/// Phase 9.5a real respawn entry point, same convention as
/// `ahci::respawn`/`netstack::respawn`.
pub fn respawn() {
    thread::spawn(xhci_driver_thread);
}

struct XhciParams {
    bus: u8,
    device: u8,
    function: u8,
    bar_phys: u64,
    bar_size: u64,
}

static mut XHCI_PARAMS: Option<XhciParams> = None;

extern "C" fn xhci_driver_thread() {
    unsafe {
        let p = (&*(&raw const XHCI_PARAMS)).as_ref().unwrap();
        crate::supervisor::mark_thread_owner(p.bus, p.device, p.function);
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, XHCI_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("XHCI_ELF_LOAD_FAILED {:?}", e);
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
                klog_info!("XHCI_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("XHCI_MAPPED phys=0x{:x} -> vaddr=0x{:x}", p.bar_phys, bar_vaddr);

        let com1_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, com1_cap) {
            Ok(()) => klog_info!("XHCI_COM1_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("XHCI_COM1_GRANT_FAILED {:?}", e);
                return;
            }
        }
        let port60_cap = driver::create_port_capability(&mut table, 0x60, 1, Rights::PORT_IO);
        let _ = driver::grant_port_access(&table, port60_cap);

        // Real DMA page (Phase 11 deliverable 2, continued): the
        // Device Context Base Address Array, Command Ring, Event Ring
        // Segment Table, and Event Ring segment all fit comfortably
        // in one real page -- same single-page DMA layout style
        // `ahci_driver`'s own module already established (a fixed set
        // of byte offsets into one real, IOMMU-contained page, not a
        // general allocator).
        let dma_phys = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DMA_VADDR,
            dma_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real ADR-006 IOMMU containment, now covering the real DMA
        // page above -- the controller's own Command Ring/Event Ring
        // reads and writes this exact page, so it must be reachable
        // through the real IOMMU domain, not just the driver's own
        // virtual mapping.
        let domain = authority::grant_device(p.bus, p.device, p.function, dma_phys, 4096)
            .unwrap_or_else(|| iommu::assign_device(p.bus, p.device, p.function, &[(dma_phys, 4096)]));
        klog_info!("XHCI_IOMMU_DOMAIN_ASSIGNED device={:02x}:{:02x}.{} domain={}", p.bus, p.device, p.function, domain.0);

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut XhciInfo;
        core::ptr::write(info_ptr, XhciInfo { bar_vaddr, dma_vaddr: DMA_VADDR, dma_phys });
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
        klog_info!("XHCI_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
    }
}
