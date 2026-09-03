//! x86_64 4-level page table index math — the exact logic
//! `kernel_rs/src/vmm.rs`'s `indices()` used, extracted as a pure function.
//! Getting this wrong silently maps to the wrong physical page instead of
//! erroring, which is exactly the kind of bug that's cheap to verify with
//! known-good test vectors and expensive to debug by booting.

pub const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

/// Splits a virtual address into (pml4_index, pdpt_index, pd_index,
/// pt_index) — each 9 bits, per the standard x86_64 4-level paging layout.
pub fn split_indices(vaddr: u64) -> (usize, usize, usize, usize) {
    let pt = ((vaddr >> 12) & 0x1ff) as usize;
    let pd = ((vaddr >> 21) & 0x1ff) as usize;
    let pdpt = ((vaddr >> 30) & 0x1ff) as usize;
    let pml4 = ((vaddr >> 39) & 0x1ff) as usize;
    (pml4, pdpt, pd, pt)
}

/// Extracts the physical frame address from a raw page-table entry value
/// (masks off the low 12 flag bits and any bits above the physical
/// address width).
pub fn frame_from_entry(entry: u64) -> u64 {
    entry & ADDR_MASK
}
