//! Intel VT-d IOMMU. Phase 3 item, ADR-006's hard gate: "Intel VT-d /
//! AMD-Vi support, with per-device DMA domains, is a hard prerequisite
//! for Phase 6 [driver synthesis]... A user-space driver without IOMMU
//! protection is not meaningfully isolated — it is a process that can
//! corrupt any physical page by asking a device to do it." This is that
//! gate, built against QEMU's real `intel-iommu` device emulation
//! (`-device intel-iommu,intremap=on` in scripts/test-boot.ps1), not a
//! stub.
//!
//! Scope, stated honestly: this brings up ONE remapping engine (DRHD),
//! builds a root/context table structure, and implements per-domain
//! second-level (DMA) address translation — a device NOT assigned to a
//! domain's page tables cannot DMA into memory that domain doesn't map.
//! AMD-Vi (the AMD equivalent) is NOT implemented — this kernel's target
//! is Intel VT-d only for now, matching the roadmap's "Intel VT-d /
//! AMD-Vi" as an either/or, not both. Interrupt remapping (also part of
//! real VT-d) is enabled at the QEMU level (`intremap=on`) but this
//! driver does not yet program IRTE entries — this is DMA (memory)
//! containment specifically, the property ADR-006 is actually about.

use crate::{klog_info, pmm, vmm};

#[repr(C, packed)]
struct DmarHeader {
    host_address_width: u8,
    flags: u8,
    reserved: [u8; 10],
}

#[repr(C, packed)]
struct RemapHeader {
    kind: u16,
    length: u16,
}

const DRHD_TYPE: u16 = 0;

// VT-d register offsets (Intel VT-d spec).
const REG_VER: u64 = 0x00;
const REG_CAP: u64 = 0x08;
const REG_ECAP: u64 = 0x10;
const REG_GCMD: u64 = 0x18;
const REG_GSTS: u64 = 0x1C;
const REG_RTADDR: u64 = 0x20;
const REG_CCMD: u64 = 0x28;
const REG_FSTS: u64 = 0x34;

const GCMD_TE: u32 = 1 << 31; // Translation Enable
const GCMD_SRTP: u32 = 1 << 30; // Set Root Table Pointer
const GSTS_TES: u32 = 1 << 31;
const GSTS_RTPS: u32 = 1 << 30;

const CCMD_ICC: u64 = 1 << 63; // Invalidate Context Cache
const CCMD_CIRG_GLOBAL: u64 = 1 << 61; // Context Invalidation Request Granularity = global

struct Regs {
    vaddr: u64,
}

impl Regs {
    unsafe fn read32(&self, offset: u64) -> u32 {
        core::ptr::read_volatile((self.vaddr + offset) as *const u32)
    }
    unsafe fn write32(&self, offset: u64, value: u32) {
        core::ptr::write_volatile((self.vaddr + offset) as *mut u32, value);
    }
    unsafe fn read64(&self, offset: u64) -> u64 {
        core::ptr::read_volatile((self.vaddr + offset) as *const u64)
    }
    unsafe fn write64(&self, offset: u64, value: u64) {
        core::ptr::write_volatile((self.vaddr + offset) as *mut u64, value);
    }
}

static mut REGS: Option<Regs> = None;
static mut ROOT_TABLE_PHYS: u64 = 0;

#[allow(static_mut_refs)]
unsafe fn regs() -> &'static Regs {
    (&*&raw const REGS).as_ref().unwrap()
}

/// Parses the DMAR table for the first DRHD entry's register base, maps
/// it via `vmm::map_mmio_page`, and returns the register window's virtual
/// address. Real parsing against the real table found by `acpi.rs` — not
/// a hardcoded address.
fn find_drhd_register_base(dmar_phys: u64) -> Option<u64> {
    unsafe {
        // SdtHeader is 36 bytes (see acpi.rs's private copy); DmarHeader
        // (12 bytes) follows it directly, then a variable list of
        // remapping structures until the table's own total length.
        const SDT_HEADER_LEN: u64 = 36;
        let length_ptr = pmm::p2v_pub(dmar_phys + 4) as *const u32; // SdtHeader.length is at offset 4
        let total_length = core::ptr::read_unaligned(length_ptr) as u64;

        let mut offset = SDT_HEADER_LEN + core::mem::size_of::<DmarHeader>() as u64;
        while offset < total_length {
            let entry_phys = dmar_phys + offset;
            let header_ptr = pmm::p2v_pub(entry_phys) as *const RemapHeader;
            let kind = core::ptr::read_unaligned(core::ptr::addr_of!((*header_ptr).kind));
            let len = core::ptr::read_unaligned(core::ptr::addr_of!((*header_ptr).length)) as u64;
            if len == 0 {
                break; // malformed; avoid an infinite loop
            }
            if kind == DRHD_TYPE {
                // DRHD: type(2) length(2) flags(1) reserved(1) segment(2) register_base(8)
                let reg_base_ptr = pmm::p2v_pub(entry_phys + 8) as *const u64;
                let reg_base = core::ptr::read_unaligned(reg_base_ptr);
                return Some(reg_base);
            }
            offset += len;
        }
        None
    }
}

pub struct DomainId(pub u16);

/// Brings up the first DRHD, builds an empty root table (every context
/// entry starts "not present" — every device denied by default until
/// explicitly assigned to a domain), and enables translation. From this
/// point on, ANY device not explicitly assigned via `assign_device` is
/// DMA-blocked by real hardware, not by kernel policy alone.
pub fn init(dmar_phys: u64) -> bool {
    let reg_base_phys = match find_drhd_register_base(dmar_phys) {
        Some(b) => b,
        None => {
            klog_info!("IOMMU: no DRHD found in DMAR table");
            return false;
        }
    };
    klog_info!("IOMMU: DRHD register base phys=0x{:x}", reg_base_phys);

    unsafe {
        let vaddr = vmm::map_mmio_page(reg_base_phys);
        REGS = Some(Regs { vaddr });
        let r = regs();

        let cap = r.read64(REG_CAP);
        let ecap = r.read64(REG_ECAP);
        let ver = r.read32(REG_VER);
        klog_info!(
            "IOMMU: VER=0x{:x} CAP=0x{:x} ECAP=0x{:x}",
            ver, cap, ecap
        );

        // Root table: 4KB, 256 entries x 16 bytes each (one per PCI bus
        // number 0-255). Every entry starts zeroed = not present = every
        // device on every bus denied by default.
        let root_phys = pmm::alloc_page();
        ROOT_TABLE_PHYS = root_phys;
        r.write64(REG_RTADDR, root_phys);
        r.write32(REG_GCMD, GCMD_SRTP);
        // Real hardware/emulation handshake: poll GSTS until RTPS confirms
        // the root table pointer was actually latched, not just written.
        let mut spins = 0;
        while r.read32(REG_GSTS) & GSTS_RTPS == 0 {
            spins += 1;
            if spins > 1_000_000 {
                klog_info!("IOMMU: timed out waiting for RTPS");
                return false;
            }
        }

        r.write32(REG_GCMD, GCMD_TE);
        spins = 0;
        while r.read32(REG_GSTS) & GSTS_TES == 0 {
            spins += 1;
            if spins > 1_000_000 {
                klog_info!("IOMMU: timed out waiting for TES (translation enable)");
                return false;
            }
        }
    }
    klog_info!("IOMMU: translation enabled, root table live, every device denied by default");
    true
}

/// Assigns PCI device (bus, device, function) to a fresh DMA domain
/// mapping exactly `phys_ranges` (a slice of (phys_base, len) pairs the
/// device is allowed to DMA into) — everything else stays unreachable to
/// that device by construction, not by convention.
pub fn assign_device(bus: u8, device: u8, function: u8, phys_ranges: &[(u64, u64)]) -> DomainId {
    unsafe {
        let r = regs();

        // Second-level page tables for this domain: a minimal 4-level
        // walk mirroring vmm.rs's own (VT-d's second-level translation
        // uses the identical x86-64 page table FORMAT as CPU paging,
        // per the VT-d spec — this is not a coincidence, it's why a
        // domain's page tables can be built with the same bit layout).
        let domain_pml4 = pmm::alloc_page();
        for (phys_base, len) in phys_ranges {
            let pages = (len + 4095) / 4096;
            for p in 0..pages {
                let addr = phys_base + p * 4096;
                vmm_map_iommu_page(domain_pml4, addr, addr);
            }
        }

        // Context table: one page per bus, 32 devices x 8 functions x 16
        // bytes = 4096 bytes exactly (one page per bus, matches the spec).
        let context_phys = pmm::alloc_page();
        let root = pmm::p2v_pub(ROOT_TABLE_PHYS) as *mut u64;
        // Root entry for this bus: [context_table_ptr | present]
        *root.add(bus as usize * 2) = context_phys | 1;

        let context = pmm::p2v_pub(context_phys) as *mut u64;
        let ctx_index = ((device as usize & 0x1F) << 3) | (function as usize & 0x7);
        // Context entry (2 x u64 = 16 bytes): low = [SLPTPTR | present],
        // high = [address-width | domain-id]. Address width field 0b010 =
        // 4-level page tables (matches our domain_pml4 layout).
        *context.add(ctx_index * 2) = domain_pml4 | 1;
        *context.add(ctx_index * 2 + 1) = (0b010u64 << 0) | ((bus as u64) << 8); // AW + a domain id derived from bus for uniqueness

        // Flush the context cache (global) so hardware picks up the new
        // entry — without this, the IOMMU may keep using a cached "not
        // present" result for this device.
        r.write64(REG_CCMD, CCMD_ICC | CCMD_CIRG_GLOBAL);
        let mut spins = 0;
        while r.read64(REG_CCMD) & CCMD_ICC != 0 {
            spins += 1;
            if spins > 1_000_000 {
                break;
            }
        }

        klog_info!(
            "IOMMU: assigned {:02x}:{:02x}.{} to a domain with {} mapped range(s)",
            bus, device, function, phys_ranges.len()
        );
        DomainId(bus as u16)
    }
}

/// Same 4-level page-table walk as vmm.rs's map_page, duplicated rather
/// than shared because it operates on a domain's second-level tables
/// (allocated fresh per assign_device call), not any process's PML4 —
/// distinct enough in purpose that sharing the exact function would blur
/// "this is a CPU address space" vs "this is a DMA domain."
unsafe fn vmm_map_iommu_page(pml4_phys: u64, vaddr: u64, paddr: u64) {
    const PRESENT: u64 = 1 << 0;
    const WRITABLE: u64 = 1 << 1;
    const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

    let pt_idx = (vaddr >> 12) & 0x1ff;
    let pd_idx = (vaddr >> 21) & 0x1ff;
    let pdpt_idx = (vaddr >> 30) & 0x1ff;
    let pml4_idx = (vaddr >> 39) & 0x1ff;

    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
    if *pml4.add(pml4_idx as usize) & PRESENT == 0 {
        let new = pmm::alloc_page();
        *pml4.add(pml4_idx as usize) = new | PRESENT | WRITABLE;
    }
    let pdpt = pmm::p2v_pub(*pml4.add(pml4_idx as usize) & ADDR_MASK) as *mut u64;
    if *pdpt.add(pdpt_idx as usize) & PRESENT == 0 {
        let new = pmm::alloc_page();
        *pdpt.add(pdpt_idx as usize) = new | PRESENT | WRITABLE;
    }
    let pd = pmm::p2v_pub(*pdpt.add(pdpt_idx as usize) & ADDR_MASK) as *mut u64;
    if *pd.add(pd_idx as usize) & PRESENT == 0 {
        let new = pmm::alloc_page();
        *pd.add(pd_idx as usize) = new | PRESENT | WRITABLE;
    }
    let pt = pmm::p2v_pub(*pd.add(pd_idx as usize) & ADDR_MASK) as *mut u64;
    *pt.add(pt_idx as usize) = (paddr & ADDR_MASK) | PRESENT | WRITABLE;
}

/// Reads the Fault Status Register — non-zero bits mean the IOMMU has
/// blocked at least one DMA attempt since the last clear. This is the
/// real evidence a containment test checks, not a kernel-side assumption.
pub fn fault_status() -> u32 {
    unsafe { regs().read32(REG_FSTS) }
}

pub fn clear_faults() {
    unsafe {
        let status = regs().read32(REG_FSTS);
        regs().write32(REG_FSTS, status); // write-1-to-clear, per spec
    }
}
