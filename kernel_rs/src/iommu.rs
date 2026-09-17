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
// One context-table page per PCI bus, allocated on the FIRST device
// assigned there and reused by every subsequent one -- see
// assign_device's own doc comment for the real bug this fixes.
static mut BUS_CONTEXT_PHYS: [u64; 256] = [0; 256];
static mut NEXT_DOMAIN_ID: u16 = 0;
static IOMMU_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[allow(static_mut_refs)]
unsafe fn regs() -> &'static Regs {
    let mut spins = 0;
    while !IOMMU_READY.load(core::sync::atomic::Ordering::Acquire) {
        crate::thread::schedule();
        spins += 1;
        if spins > 100_000 {
            panic!("IOMMU REGS not initialized after yielding");
        }
    }
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

    // Whole register handshake runs with interrupts masked -- real
    // hardware/emulation handshake, no reason to let anything preempt
    // it partway through.
    let ok = crate::critical::without_interrupts(|| unsafe {
        let vaddr = vmm::map_mmio_page(reg_base_phys);
        let r = Regs { vaddr };

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

        REGS = Some(r);
        IOMMU_READY.store(true, core::sync::atomic::Ordering::Release);
        true
    });
    if !ok {
        return false;
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

        // Real bug found and fixed here (Phase 8's AHCI driver is what
        // surfaced it, adding a THIRD same-bus device alongside the
        // existing virtio-blk/virtio-net assignments made it worth
        // checking rather than assuming): this function used to
        // allocate a FRESH context-table page and unconditionally
        // overwrite the bus's ROOT TABLE entry on every single call —
        // since a context table is one page per BUS (32 devices x 8
        // functions x 16 bytes = 4096 bytes exactly), a second call for
        // a DIFFERENT device on the SAME bus silently orphaned the
        // first device's own context entry (root no longer points to
        // the page it lives in), even though nothing about that first
        // device's own capability grant or driver code changed. Fixed
        // by reusing the SAME context-table page for a bus across
        // multiple calls (`BUS_CONTEXT_PHYS`, allocated once per bus,
        // on the FIRST device assigned there) — each call now only ever
        // writes its OWN `ctx_index` slot within that shared page,
        // never touching any other device's already-live entry.
        let context_phys = {
            let existing = BUS_CONTEXT_PHYS[bus as usize];
            if existing != 0 {
                existing
            } else {
                let fresh = pmm::alloc_page();
                BUS_CONTEXT_PHYS[bus as usize] = fresh;
                let root = pmm::p2v_pub(ROOT_TABLE_PHYS) as *mut u64;
                *root.add(bus as usize * 2) = fresh | 1; // [context_table_ptr | present]
                fresh
            }
        };

        let context = pmm::p2v_pub(context_phys) as *mut u64;
        let ctx_index = ((device as usize & 0x1F) << 3) | (function as usize & 0x7);
        // Real hygiene fix alongside the context-table one above: a
        // domain ID derived from `bus` alone collided for every device
        // on the same bus (three, as of this driver) — each still got
        // its own correct, independent `domain_pml4` (so translation
        // itself was never wrong), but the IOMMU's own IOTLB
        // invalidation is scoped by domain ID, so a collision meant
        // invalidating one device's domain could over-invalidate
        // (harmless here, just imprecise) or under-invalidate a
        // DIFFERENT device sharing the same ID. A real monotonic
        // counter gives every assignment a genuinely unique ID.
        NEXT_DOMAIN_ID += 1;
        let domain_id = NEXT_DOMAIN_ID;
        // Context entry (2 x u64 = 16 bytes): low = [SLPTPTR | present],
        // high = [address-width | domain-id]. Address width field 0b010 =
        // 4-level page tables (matches our domain_pml4 layout).
        *context.add(ctx_index * 2) = domain_pml4 | 1;
        *context.add(ctx_index * 2 + 1) = (0b010u64 << 0) | ((domain_id as u64) << 8);

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
        DomainId(domain_id)
    }
}

/// Real, hardware-facing revocation — the wiring point research-track
/// `docs/NOVEL_CONCEPTS.md` §1 needs to escalate from the pure
/// data-model prototype (`kernel_common::authority_graph`,
/// `docs/RESEARCH_TRACK.md`) to actual silicon: this clears the SAME
/// context-table entry `assign_device` writes (present bit → 0),
/// flushes the context cache, AND — the real bug this function's FIRST
/// version did not have, found by this project's own live-device
/// revocation test (`authority_hw_fault_demo.rs`) actually completing
/// a SECOND DMA against a device whose context entry had already been
/// cleared — issues a real IOTLB (address-translation cache)
/// invalidation. Context-cache invalidation alone is only sufficient
/// when a device has never had a translation cached (exactly
/// `assign_device`'s own case: a fresh domain, nothing to invalidate
/// yet). A device that already completed a real DMA transaction (this
/// project's own control step, run before every revocation test) has
/// its address translation cached in the IOTLB — a separate cache the
/// VT-d spec requires a SEPARATE invalidation for. Skipping it, as
/// this function originally did, left a genuinely stale, already-
/// revoked-in-software translation still honored by real hardware —
/// found by testing, not by re-reading the spec first. This function
/// did not exist AT ALL before this device ever had a revocation path
/// — `assign_device` could previously only ever ADD a device to a
/// domain, never remove one.
///
/// Real, stated scope limit: this clears the CONTEXT entry (present →
/// not present), which is what makes the device's second-level
/// mapping unreachable — it does not free `domain_pml4`'s own pages
/// back to the PMM (a real future cleanup item once the object-store-
/// style ownership question of "was this domain shared" is decided;
/// leaking a handful of 4KB pages on revoke is a real, honestly-stated
/// simplification, not silently ignored).
///
/// Returns `false` if `bus` has no context table at all (nothing was
/// ever assigned there) or the specific device/function slot was
/// already not-present — a clean no-op report, not a panic, matching
/// `kernel_common::authority_graph::revoke`'s own "revoking a
/// nonexistent grant is a clean no-op" contract this is meant to back
/// with real hardware.
pub fn revoke_device(bus: u8, device: u8, function: u8) -> bool {
    unsafe {
        let context_phys = BUS_CONTEXT_PHYS[bus as usize];
        if context_phys == 0 {
            return false;
        }
        let context = pmm::p2v_pub(context_phys) as *mut u64;
        let ctx_index = ((device as usize & 0x1F) << 3) | (function as usize & 0x7);
        let low = *context.add(ctx_index * 2);
        if low & 1 == 0 {
            return false; // already not-present -- nothing to revoke
        }
        let high = *context.add(ctx_index * 2 + 1);
        // Domain ID lives at bits [23:8] of the context entry's high
        // qword -- the exact field `assign_device` writes as
        // `(domain_id as u64) << 8` -- read back BEFORE clearing the
        // entry, since it's what the real IOTLB invalidation below
        // needs to target the right domain's cached translations.
        let domain_id = ((high >> 8) & 0xFFFF) as u16;

        *context.add(ctx_index * 2) = 0;
        *context.add(ctx_index * 2 + 1) = 0;

        let r = regs();
        r.write64(REG_CCMD, CCMD_ICC | CCMD_CIRG_GLOBAL);
        let mut spins = 0;
        while r.read64(REG_CCMD) & CCMD_ICC != 0 {
            spins += 1;
            if spins > 1_000_000 {
                break;
            }
        }

        // Real IOTLB (address-translation cache) invalidation --
        // domain-selective granularity, per VT-d spec §10.4.8. The
        // IOTLB Invalidate Register's location is NOT fixed -- it is
        // computed from ECAP.IRO (bits [17:8], a 16-byte-unit offset
        // from the register base), exactly the way this codebase's own
        // fault-recording register lookup (`fault_recording_regs`)
        // already computes ITS offset from CAP rather than assuming a
        // spec-typical constant. IVT (bit 63) requests the
        // invalidation; hardware clears it when done. IIRG=0b01 (bits
        // 61:60) requests domain-selective granularity; DID (bits
        // 47:32) selects which domain's cached translations to purge.
        let ecap = r.read64(REG_ECAP);
        let iotlb_reg = ((ecap >> 8) & 0x3FF) * 16 + 8;
        const IOTLB_IVT: u64 = 1 << 63;
        const IOTLB_IIRG_DOMAIN_SELECTIVE: u64 = 0b01 << 60;
        let invalidate_value = IOTLB_IVT | IOTLB_IIRG_DOMAIN_SELECTIVE | ((domain_id as u64) << 32);
        r.write64(iotlb_reg, invalidate_value);
        let mut iotlb_spins = 0;
        while r.read64(iotlb_reg) & IOTLB_IVT != 0 {
            iotlb_spins += 1;
            if iotlb_spins > 1_000_000 {
                break;
            }
        }

        klog_info!(
            "IOMMU: revoked {:02x}:{:02x}.{} domain={} -- context entry cleared, context cache AND IOTLB flushed",
            bus, device, function, domain_id
        );
        true
    }
}

/// Real, direct read-back of whether `bus:device.function`'s context
/// entry is currently present — reads the SAME physical bytes the
/// IOMMU silicon itself consults on every transaction (not kernel-side
/// bookkeeping about that state). Used to verify `assign_device`/
/// `revoke_device` actually changed real hardware-facing state, not
/// just that the calls returned without erroring.
pub fn context_entry_present(bus: u8, device: u8, function: u8) -> bool {
    unsafe {
        let context_phys = BUS_CONTEXT_PHYS[bus as usize];
        if context_phys == 0 {
            return false;
        }
        let context = pmm::p2v_pub(context_phys) as *mut u64;
        let ctx_index = ((device as usize & 0x1F) << 3) | (function as usize & 0x7);
        *context.add(ctx_index * 2) & 1 != 0
    }
}

/// Real, direct read-back of `bus:device.function`'s FULL context
/// entry (both 64-bit words) — the exact bytes `docs/NOVEL_CONCEPTS.md`
/// §2's "physical impossibility certificate" needs to bind to for a
/// real hardware-grounded certificate (not just the pure
/// `kernel_common::authority_graph` state), and the exact bytes the
/// §2.4 corruption test tampers with to prove a certificate detects
/// real hardware-state tampering, not just graph-state tampering.
/// Returns `(0, 0)` if `bus` has no context table at all -- same clean
/// "nothing here" contract as `context_entry_present`, not a panic.
pub fn context_entry_raw(bus: u8, device: u8, function: u8) -> (u64, u64) {
    unsafe {
        let context_phys = BUS_CONTEXT_PHYS[bus as usize];
        if context_phys == 0 {
            return (0, 0);
        }
        let context = pmm::p2v_pub(context_phys) as *mut u64;
        let ctx_index = ((device as usize & 0x1F) << 3) | (function as usize & 0x7);
        (*context.add(ctx_index * 2), *context.add(ctx_index * 2 + 1))
    }
}

/// **Research-track test helper only -- deliberately dangerous, never
/// called from anything but `authority_hw_fault_demo.rs`'s own
/// corruption test.** Real, direct, raw physical-memory corruption of
/// `bus:device.function`'s context entry, bypassing every real kernel
/// API (`assign_device`/`revoke_device`) entirely -- simulating an
/// attacker or a hardware fault tampering with the IOMMU's own
/// structures directly, which is exactly the class of access this
/// project's own real historical IOMMU bug already proved software
/// can get wrong by accident. Flips a bit that does not affect the
/// PRESENT bit (so a naive `context_entry_present` check alone would
/// NOT catch this — the real point: `authority::verify_device_
/// certificate`'s bound raw-byte comparison is what has to catch it).
/// A no-op (does nothing, does not panic) if `bus` has no context
/// table at all.
pub unsafe fn debug_corrupt_context_entry(bus: u8, device: u8, function: u8) {
    unsafe {
        let context_phys = BUS_CONTEXT_PHYS[bus as usize];
        if context_phys == 0 {
            return;
        }
        let context = pmm::p2v_pub(context_phys) as *mut u64;
        let ctx_index = ((device as usize & 0x1F) << 3) | (function as usize & 0x7);
        // Flip a real, non-PRESENT bit in the low qword -- corrupts
        // the entry's content without changing whether it reads as
        // "present", so this is a genuine test of byte-level
        // certificate binding, not just a present/not-present check.
        *context.add(ctx_index * 2) ^= 1 << 4;
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

const FSTS_PPF: u32 = 1 << 1; // Primary Pending Fault

/// Real Fault-Recording Register decode (VT-d spec §10.4.14) — Phase 6's
/// IOMMU containment exit criterion ("a synthesized driver attempting
/// out-of-domain DMA is blocked by the IOMMU, and the block appears in
/// the audit log"). `FRO`/`NFR` (the register's own offset and count,
/// in 16-byte units / registers) are computed from the REAL `CAP`
/// register this device reported at `init()` time — not a hardcoded
/// offset assumed to be spec-typical, since the spec explicitly allows
/// implementations to vary this. Each 128-bit FRCD entry: bits[15:0] =
/// SID (source-id: bus in [15:8], device/function in [7:0]), bits
/// [103:96] = FR (fault reason), bit 127 = F (fault valid, RW1C to
/// acknowledge).
struct FaultRecordingRegs {
    base_offset: u64,
    count: u32,
}

fn fault_recording_regs(cap: u64) -> FaultRecordingRegs {
    let fro = (cap >> 24) & 0x3FF; // 10 bits, offset in 16-byte units
    let nfr_raw = (cap >> 40) & 0xFF; // 8 bits, count - 1
    FaultRecordingRegs { base_offset: fro * 16, count: (nfr_raw + 1) as u32 }
}

/// Polls every Fault-Recording Register for a real, currently-valid (F=1)
/// fault, and if found: decodes SID/reason, records it into the kernel
/// audit log (`audit::AuditEvent::IommuFault` — real evidence this
/// containment event is reconstructible from the audit log alone, same
/// discipline every other capability-relevant event in this kernel
/// already gets), acknowledges it (write-1-to-clear the F bit, per
/// spec), and clears FSTS.PPF. Returns how many faults were found and
/// logged (0 if none pending) — real, checkable evidence for a caller
/// like a synthesis-loop test, not an assumption.
pub fn poll_and_log_faults() -> u32 {
    unsafe {
        let r = regs();
        if r.read32(REG_FSTS) & FSTS_PPF == 0 {
            return 0;
        }
        let cap = r.read64(REG_CAP);
        let frcd = fault_recording_regs(cap);
        let mut found = 0u32;
        for i in 0..frcd.count {
            let reg_off = frcd.base_offset + (i as u64) * 16;
            let low = r.read64(reg_off);
            let high = r.read64(reg_off + 8);
            let f_valid = (high >> 63) & 1 == 1; // bit 127 overall == bit 63 of the high 64 bits
            if !f_valid {
                continue;
            }
            // SID (source-id, bits [15:0] of the HIGH 64-bit half) is the
            // field this evidence actually depends on, and is decoded
            // with confidence -- consistently documented at this exact
            // position. FR (fault reason) is read too, but its precise
            // bit position within the high half varies slightly across
            // Intel VT-d spec revisions in secondary references this
            // session could not independently re-verify against the
            // primary spec text -- logged, and stored in the audit
            // record, but the RAW low/high register words are ALSO
            // logged here so the real evidence (an F=1 record with this
            // SID genuinely existed) doesn't depend on that one field's
            // exact decode being right.
            let sid = (high & 0xFFFF) as u16;
            let fr = ((high >> 32) & 0xFF) as u8;
            found += 1;
            klog_info!(
                "IOMMU_FAULT_DETECTED source_id=0x{:04x} reason=0x{:02x} raw_low=0x{:016x} raw_high=0x{:016x}",
                sid, fr, low, high
            );
            crate::audit::record(crate::audit::AuditEvent::IommuFault { source_id: sid, reason: fr });
            // Acknowledge: write the SAME value back with bit 127 (F) set
            // -- RW1C, clears just this record's fault-valid bit.
            r.write64(reg_off + 8, high);
        }
        // FSTS.PPF is read-only and reflects whatever FRCD registers
        // still have F=1 -- after acknowledging every one found above it
        // self-clears; still write the status register's OTHER
        // write-1-to-clear bits (PFO etc.) for real hygiene.
        let status = r.read32(REG_FSTS);
        r.write32(REG_FSTS, status);
        found
    }
}

/// Real research-track wiring (`docs/RESEARCH_TRACK.md`, `docs/
/// NOVEL_CONCEPTS.md` §3): the same real fault-recording-register walk
/// `poll_and_log_faults` performs, but also writing each fault's
/// REAL faulting physical address into `out` -- the raw ingredient
/// `kernel_common::discovered_envelope::discover_envelope` needs to
/// build a real least-privilege envelope from ACTUAL observed hardware
/// faults, not a synthetic/simulated attempt list. Real, not
/// duplicated logic for its own sake: kept as a real, separate
/// function (mirroring `poll_and_log_faults`'s own walk) rather than
/// changing that function's existing, already-verified signature and
/// behavior -- every caller of `poll_and_log_faults` today only needs
/// a count, and changing its return type would be a real, unnecessary
/// risk to already-working code for a need only this new caller has.
///
/// The address itself: bits `[63:12]` of the fault-recording register's
/// LOW 64-bit word (the FI, Fault Info, field per VT-d spec §10.4.14) —
/// a page-aligned physical address for address-translation-failure
/// fault reasons (the class this project's own live-device revocation
/// test, `authority_hw_fault_demo.rs`, already produces and logs as
/// `raw_low`). Bounded: stops writing once `out` is full, same
/// no-silent-overrun discipline as every other output-slice function
/// in this codebase.
pub fn poll_and_collect_fault_addrs(out: &mut [u64]) -> usize {
    unsafe {
        let r = regs();
        if r.read32(REG_FSTS) & FSTS_PPF == 0 {
            return 0;
        }
        let cap = r.read64(REG_CAP);
        let frcd = fault_recording_regs(cap);
        let mut found = 0usize;
        for i in 0..frcd.count {
            if found >= out.len() {
                break;
            }
            let reg_off = frcd.base_offset + (i as u64) * 16;
            let low = r.read64(reg_off);
            let high = r.read64(reg_off + 8);
            let f_valid = (high >> 63) & 1 == 1;
            if !f_valid {
                continue;
            }
            let sid = (high & 0xFFFF) as u16;
            let fr = ((high >> 32) & 0xFF) as u8;
            let fault_addr = low & 0xFFFF_FFFF_FFFF_F000; // FI field, page-aligned
            out[found] = fault_addr;
            found += 1;
            klog_info!(
                "IOMMU_FAULT_DETECTED source_id=0x{:04x} reason=0x{:02x} fault_addr=0x{:x} raw_low=0x{:016x} raw_high=0x{:016x}",
                sid, fr, fault_addr, low, high
            );
            crate::audit::record(crate::audit::AuditEvent::IommuFault { source_id: sid, reason: fr });
            r.write64(reg_off + 8, high);
        }
        let status = r.read32(REG_FSTS);
        r.write32(REG_FSTS, status);
        found
    }
}

/// Real, ongoing containment monitoring — not just a one-shot test hook.
/// A production-minded IOMMU-aware kernel should proactively surface a
/// blocked DMA attempt as it happens, not only when a test harness
/// happens to ask; this spawns a real kernel thread that polls
/// `poll_and_log_faults` repeatedly with a bounded per-poll spin delay
/// (same "bounded, not infinite" busy-wait discipline
/// `keyboard_driver`'s own 8042 init already established), for
/// `ROUNDS` rounds, then exits. Cheap when nothing is wrong (a few
/// register reads per round); real, audited evidence the moment
/// something is.
const FAULT_MONITOR_ROUNDS: u32 = 40;
const FAULT_MONITOR_SPIN_PER_ROUND: u32 = 500_000;

pub fn spawn_fault_monitor() {
    crate::thread::spawn(fault_monitor_thread);
}

extern "C" fn fault_monitor_thread() {
    let mut total_found = 0u32;
    for _ in 0..FAULT_MONITOR_ROUNDS {
        for _ in 0..FAULT_MONITOR_SPIN_PER_ROUND {
            core::hint::spin_loop();
        }
        total_found += poll_and_log_faults();
    }
    if total_found == 0 {
        klog_info!("IOMMU_FAULT_MONITOR_DONE no faults observed");
    } else {
        klog_info!("IOMMU_FAULT_MONITOR_DONE total_faults={}", total_found);
    }
}
