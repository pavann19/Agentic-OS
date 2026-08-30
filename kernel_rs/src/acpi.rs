//! ACPI table discovery. Phase 3 — needed specifically to find the DMAR
//! table (IOMMU register base), but written as a real, general ACPI
//! walker since DMAR is not the only table later phases will need
//! (MADT for APIC topology, MCFG for PCIe ECAM, FADT for power
//! management — none built yet, this is the mechanism they'll all reuse).
//!
//! `boot_rs`'s RSDP is a raw physical address handed through `BootInfo`;
//! everything here reads it through the kernel's direct-map window
//! (`pmm::p2v_pub`), never a bare physical-address dereference — same
//! discipline as every other post-VMM-init physical access in this
//! kernel.

use crate::{klog_info, pmm};

#[repr(C, packed)]
struct Rsdp20 {
    signature: [u8; 8], // "RSD PTR "
    checksum: u8,
    oem_id: [u8; 6],
    revision: u8,
    rsdt_address: u32,
    length: u32,
    xsdt_address: u64,
    extended_checksum: u8,
    reserved: [u8; 3],
}

#[repr(C, packed)]
struct SdtHeader {
    signature: [u8; 4],
    length: u32,
    revision: u8,
    checksum: u8,
    oem_id: [u8; 6],
    oem_table_id: [u8; 8],
    oem_revision: u32,
    creator_id: u32,
    creator_revision: u32,
}

fn checksum_ok(phys: u64, len: usize) -> bool {
    unsafe {
        let ptr = pmm::p2v_pub(phys);
        let mut sum: u8 = 0;
        for i in 0..len {
            sum = sum.wrapping_add(*ptr.add(i));
        }
        sum == 0
    }
}

/// Validates the RSDP and returns the XSDT's physical address (ACPI 2.0+
/// only — this kernel doesn't fall back to RSDT's 32-bit pointers/RSDT
/// entry format; OVMF and every real machine this targets provides
/// ACPI 2.0+). Returns None if the RSDP is missing or fails checksum.
pub fn validate_rsdp(rsdp_phys: u64) -> Option<u64> {
    if rsdp_phys == 0 {
        klog_info!("ACPI: no RSDP provided by bootloader");
        return None;
    }
    unsafe {
        let rsdp = pmm::p2v_pub(rsdp_phys) as *const Rsdp20;
        let sig = (*rsdp).signature;
        if &sig != b"RSD PTR " {
            klog_info!("ACPI: RSDP signature mismatch");
            return None;
        }
        // First 20 bytes (the ACPI 1.0 portion) must sum to 0 on their own;
        // the full ACPI 2.0+ structure must ALSO sum to 0 across its whole
        // length — two separate checksums, per spec.
        if !checksum_ok(rsdp_phys, 20) {
            klog_info!("ACPI: RSDP v1 checksum failed");
            return None;
        }
        let length = (*rsdp).length;
        if (*rsdp).revision >= 2 && !checksum_ok(rsdp_phys, length as usize) {
            klog_info!("ACPI: RSDP v2 extended checksum failed");
            return None;
        }
        let xsdt = (*rsdp).xsdt_address;
        klog_info!("ACPI: RSDP valid, revision={}, xsdt=0x{:x}", (*rsdp).revision, xsdt);
        Some(xsdt)
    }
}

/// Walks the XSDT looking for a table whose 4-byte signature matches.
/// Returns its physical address if found and its own checksum is valid.
pub fn find_table(xsdt_phys: u64, signature: &[u8; 4]) -> Option<u64> {
    unsafe {
        let header = pmm::p2v_pub(xsdt_phys) as *const SdtHeader;
        let total_len = (*header).length as usize;
        let entries_len = total_len - core::mem::size_of::<SdtHeader>();
        let entry_count = entries_len / 8; // XSDT entries are 8-byte physical pointers
        let entries = (xsdt_phys + core::mem::size_of::<SdtHeader>() as u64) as *const u64;
        // entries is a physical address used as a pointer above only for
        // arithmetic — dereference goes through p2v_pub properly below.
        let entries_v = pmm::p2v_pub(entries as u64) as *const u64;

        for i in 0..entry_count {
            let table_phys = core::ptr::read_unaligned(entries_v.add(i));
            let table_header = pmm::p2v_pub(table_phys) as *const SdtHeader;
            let table_sig = (*table_header).signature;
            if &table_sig == signature {
                let table_len = (*table_header).length as usize;
                if checksum_ok(table_phys, table_len) {
                    return Some(table_phys);
                }
                klog_info!("ACPI: table {:?} found but checksum failed", core::str::from_utf8(signature).unwrap_or("?"));
            }
        }
        None
    }
}

pub fn init(rsdp_phys: u64) -> Option<u64> {
    let xsdt = validate_rsdp(rsdp_phys)?;
    Some(xsdt)
}
