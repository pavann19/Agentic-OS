//! Phase 10 (`docs/ROADMAP.md` §5): kernel-side spawn code for the real
//! network-stack process — same real capability-mediation discipline
//! as every driver in this kernel (`ahci.rs`/`e1000.rs`/`nvme.rs`): the
//! kernel discovers the device, grants exactly the MMIO/DMA capabilities
//! the driver needs (through a real ADR-006 IOMMU domain), and the
//! ring-3 process (`user_rs/netstack_driver`) speaks the actual wire
//! protocols itself, unmediated after that one-time grant.
//!
//! Real, disclosed difference from every earlier driver's kernel-side
//! spawn code: this is the FIRST driver whose real DMA needs cannot fit
//! in one page (a real descriptor ring plus several real, independently
//! addressed frame buffers — see `user_rs/netstack_driver`'s own
//! `NetInfo` doc comment for exactly why they must be SEPARATE
//! allocations, not one contiguous blob, given `pmm::alloc_page()`'s
//! own no-cross-call-contiguity guarantee). This module allocates each
//! one as its own real page, maps all of them into the new process's
//! address space at consecutive virtual addresses, and passes the
//! REAL, individual physical address of each into `NetInfo` — and
//! grants the real IOMMU domain EVERY one of those real physical
//! ranges, not just the first, so the device is contained to exactly
//! the memory it can actually reach, matching ADR-006 unconditionally
//! regardless of how many separate allocations that takes.
//!
//! Mutually exclusive with `e1000.rs`'s own driver by construction —
//! see `main.rs`'s own call site: both would otherwise race for the
//! SAME PCI device's BAR/DMA, corrupting each other's state. Gated
//! behind the `network_stack` Cargo feature (off by default, matching
//! this project's own `fault_test_*`/`research_authority_hw_demo`
//! precedent) — `scripts/test-network.ps1` builds and boots the
//! feature-enabled kernel; the default boot still runs `e1000.rs`'s
//! own, already-evidenced Phase 8 demo unchanged.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, iommu, klog_info, pci, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const DESC_VADDR: u64 = 0x0000_0000_0053_0000;
const RX_BUF_VADDR: u64 = 0x0000_0000_0054_0000; // 8 pages: 0x54_0000 .. 0x5C_0000
const TX_BUF_VADDR: u64 = 0x0000_0000_005C_0000;

const RX_RING_ENTRIES: usize = 8;

#[repr(C)]
struct NetInfo {
    bar_vaddr: u64,
    desc_vaddr: u64,
    desc_phys: u64,
    rx_buf_vaddr: u64,
    rx_buf_phys: [u64; RX_RING_ENTRIES],
    tx_buf_vaddr: u64,
    tx_buf_phys: u64,
}

static NETSTACK_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/netstack_driver/target/x86_64-unknown-none/release/netstack_driver");

/// Same real e1000-class device `e1000.rs` already finds — this module
/// is what actually OWNS it once the `network_stack` feature is on.
fn find_nic(devices: &[pci::PciDevice]) -> Option<pci::PciDevice> {
    devices.iter().copied().find(|d| d.vendor_id == 0x8086 && d.device_id == 0x100E)
}

pub fn spawn_if_present(devices: &[pci::PciDevice]) {
    let Some(dev) = find_nic(devices) else {
        klog_info!("NETSTACK: no e1000-class NIC found (nothing to spawn)");
        return;
    };
    klog_info!("NETSTACK_FOUND {:02x}:{:02x}.{}", dev.bus, dev.device, dev.function);

    let bar = match pci::read_bar(dev.bus, dev.device, dev.function, 0) {
        Some(b) => b,
        None => {
            klog_info!("NETSTACK: BAR0 unreadable/unsupported");
            return;
        }
    };
    klog_info!("NETSTACK_BAR0 phys=0x{:x} size={}", bar.phys_addr, bar.size);

    unsafe {
        NETSTACK_PARAMS = Some(NetstackParams {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            bar_phys: bar.phys_addr,
            bar_size: bar.size,
        });
    }
    // Phase 9.5a reuse: the real device identity comes from the ACTUAL
    // discovered PCI device (dev.bus/device/function), never a
    // hardcoded guess -- registered here, once, right after this
    // device's identity is known, so supervisor.rs can trace a later
    // real fault on this process back to this exact device.
    crate::supervisor::register(dev.bus, dev.device, dev.function, respawn);
    thread::spawn(netstack_driver_thread);
}

/// Phase 9.5a real respawn entry point, matching `ahci::respawn`'s own
/// convention exactly — registered via `supervisor::register` right
/// after the first spawn, called by `supervisor::on_process_killed`
/// when a real fault kills the network-stack process and a restart is
/// approved. Re-reads the SAME stored `NETSTACK_PARAMS` (this device's
/// BAR/identity don't change across a restart).
pub fn respawn() {
    thread::spawn(netstack_driver_thread);
}

struct NetstackParams {
    bus: u8,
    device: u8,
    function: u8,
    bar_phys: u64,
    bar_size: u64,
}

static mut NETSTACK_PARAMS: Option<NetstackParams> = None;

extern "C" fn netstack_driver_thread() {
    unsafe {
        let p = (&*(&raw const NETSTACK_PARAMS)).as_ref().unwrap();
        // Phase 9.5a: record ownership BEFORE anything that could fault
        // -- same real discipline `ahci.rs`'s own driver thread already
        // established (see its own comment on why this must be as
        // early as possible).
        crate::supervisor::mark_thread_owner(p.bus, p.device, p.function);

        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, NETSTACK_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("NETSTACK_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        // Real bug found and fixed via an actual live crash (a page
        // fault, instruction fetch at address 0 -- a real stack
        // overflow, caught live by Phase 9.5a's own supervisor, which
        // correctly restarted this process three times before
        // quarantining it exactly as designed): the single, 4KB page
        // every OTHER driver's own ring-3 stack gets (ahci.rs, e1000.rs)
        // was real and sufficient for their own shallower call chains,
        // but netstack_driver's real protocol stack (ARP resolve ->
        // DNS resolve -> UDP build -> checksum, each its own stack
        // frame, some with real local buffers) genuinely needs more.
        // DRIVER_STACK_PAGES real, separately-allocated pages (see
        // this module's own doc comment on why buffers don't need
        // physical contiguity, same reasoning applies to a stack:
        // hardware only cares about the VIRTUAL addresses a stack
        // pointer walks, not physical layout), mapped at consecutive
        // virtual addresses so the ring-3 process sees one ordinary
        // contiguous stack.
        const DRIVER_STACK_PAGES: u64 = 4; // 16KB -- real headroom for this stack's genuinely deeper real call chains (ARP resolve -> DNS resolve -> UDP/IPv4 build -> checksum) than ahci.rs/e1000.rs's own single-page drivers ever needed
        for i in 0..DRIVER_STACK_PAGES {
            let stack_page = pmm::alloc_page();
            vmm::map_page_in(space, DRIVER_STACK_VADDR + i * 4096, stack_page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        }

        let mut table = CapabilityTable::new();
        let cap = driver::create_mmio_capability(&mut table, p.bar_phys, p.bar_size, Rights::MAP);
        let bar_vaddr = match driver::map_mmio(&table, cap, space) {
            Ok(v) => v,
            Err(e) => {
                klog_info!("NETSTACK_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("NETSTACK_MAPPED phys=0x{:x} -> vaddr=0x{:x}", p.bar_phys, bar_vaddr);

        let com1_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, com1_cap) {
            Ok(()) => klog_info!("NETSTACK_COM1_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("NETSTACK_COM1_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // Real descriptor-ring page -- ONE real allocation, exactly as
        // large as the RX+TX rings actually need (see
        // netstack_driver's own RX_DESC_OFF/TX_DESC_OFF layout).
        let desc_phys = pmm::alloc_page();
        vmm::map_page_in(space, DESC_VADDR, desc_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        // Real, SEPARATE per-buffer pages -- see this module's own doc
        // comment for why: pmm::alloc_page() gives no cross-call
        // contiguity guarantee, and each RxDesc/TxDesc addresses its
        // buffer independently anyway, so there is no reason to need
        // contiguity here. Mapped at consecutive VIRTUAL addresses so
        // the ring-3 driver can still reach buffer `i` via a simple
        // `rx_buf_vaddr + i*4096`, even though the underlying physical
        // pages are scattered.
        let mut rx_buf_phys = [0u64; RX_RING_ENTRIES];
        let mut iommu_ranges: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
        iommu_ranges.push((desc_phys, 4096));
        for i in 0..RX_RING_ENTRIES {
            let phys = pmm::alloc_page();
            rx_buf_phys[i] = phys;
            vmm::map_page_in(space, RX_BUF_VADDR + (i as u64) * 4096, phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
            iommu_ranges.push((phys, 4096));
        }
        let tx_buf_phys = pmm::alloc_page();
        vmm::map_page_in(space, TX_BUF_VADDR, tx_buf_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        iommu_ranges.push((tx_buf_phys, 4096));

        // Real ADR-006 IOMMU containment -- EVERY real physical page
        // this device can actually DMA into, not just the first one.
        // A device granted a domain covering fewer pages than it
        // actually uses is not really contained; this is why
        // `iommu_ranges` is built up above rather than hand-typed.
        let domain = iommu::assign_device(p.bus, p.device, p.function, &iommu_ranges);
        klog_info!(
            "NETSTACK_IOMMU_DOMAIN_ASSIGNED device={:02x}:{:02x}.{} domain={} ranges={}",
            p.bus, p.device, p.function, domain.0, iommu_ranges.len()
        );

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut NetInfo;
        core::ptr::write(info_ptr, NetInfo {
            bar_vaddr,
            desc_vaddr: DESC_VADDR,
            desc_phys,
            rx_buf_vaddr: RX_BUF_VADDR,
            rx_buf_phys,
            tx_buf_vaddr: TX_BUF_VADDR,
            tx_buf_phys,
        });
        vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("NETSTACK_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + DRIVER_STACK_PAGES * 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + DRIVER_STACK_PAGES * 4096);
    }
}
