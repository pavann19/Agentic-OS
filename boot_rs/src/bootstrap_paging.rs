//! Builds the TEMPORARY page tables the bootloader installs right before
//! jumping to the kernel. "Temporary" per `docs/ROADMAP.md`'s virtual
//! memory policy — this is the "Temporary low identity mapping only during
//! early transition" step, not the kernel's real VMM. Its only jobs: (1)
//! keep the bootloader's own currently-executing code/data reachable across
//! the CR3 switch (everything the UEFI memory map reports, identity-mapped
//! with 2MB pages — cheap, and PMM allocations can land anywhere in that
//! range), (2) map the kernel's higher-half virtual range so its entry
//! point is valid to jump to. `kernel_rs::vmm::init()` replaces this
//! entirely with permission-correct, purpose-built tables and switches CR3
//! again almost immediately after entry — this table's job is just to
//! survive that one jump.

use crate::uefi::{EfiMemoryDescriptor, PhysicalAddress};

const PAGE_PRESENT: u64 = 1 << 0;
const PAGE_WRITABLE: u64 = 1 << 1;
const PAGE_HUGE: u64 = 1 << 7; // PS bit: this PD entry maps 2MB directly.
const ADDR_MASK_2MB: u64 = 0x000F_FFFF_FFE0_0000;
const ADDR_MASK_4K: u64 = 0x000F_FFFF_FFFF_F000;

/// Bump allocator over a pre-reserved, page-aligned physical pool —
/// allocated via AllocatePages while boot services were still live (see
/// loader.rs), since nothing can allocate memory after ExitBootServices.
/// Each `next()` call hands out one zeroed 4K page for a page-table level.
pub struct TablePool {
    base: u64,
    used_pages: u64,
    total_pages: u64,
}

impl TablePool {
    pub fn new(base: PhysicalAddress, total_pages: u64) -> Self {
        Self {
            base,
            used_pages: 0,
            total_pages,
        }
    }

    unsafe fn next(&mut self) -> u64 {
        if self.used_pages >= self.total_pages {
            // Bootstrap-only scratch pool exhausted — this is a fixed-size
            // budget picked generously for a QEMU-scale RAM span (see
            // loader.rs). Rather than silently mis-map memory or corrupt
            // adjacent pages, halt loudly on serial: this is a bootstrap
            // capacity bug, not a runtime condition to recover from.
            crate::serial::write_str("BOOT_ERROR: bootstrap page-table pool exhausted\n");
            loop {
                core::arch::asm!("pause", options(nomem, nostack));
            }
        }
        let addr = self.base + self.used_pages * 4096;
        self.used_pages += 1;
        core::ptr::write_bytes(addr as *mut u8, 0, 4096);
        addr
    }
}

fn indices(vaddr: u64) -> (usize, usize, usize) {
    let pd = ((vaddr >> 21) & 0x1ff) as usize;
    let pdpt = ((vaddr >> 30) & 0x1ff) as usize;
    let pml4 = ((vaddr >> 39) & 0x1ff) as usize;
    (pml4, pdpt, pd)
}

fn indices_4k(vaddr: u64) -> (usize, usize, usize, usize) {
    let pt = ((vaddr >> 12) & 0x1ff) as usize;
    let (pml4, pdpt, pd) = indices(vaddr);
    (pml4, pdpt, pd, pt)
}

unsafe fn walk_to_pd(pool: &mut TablePool, pml4_phys: u64, vaddr: u64) -> *mut u64 {
    let (i4, i3, i2) = indices(vaddr);
    let pml4 = pml4_phys as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        *pml4.add(i4) = pool.next() | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pdpt = (*pml4.add(i4) & ADDR_MASK_4K) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        *pdpt.add(i3) = pool.next() | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pd = (*pdpt.add(i3) & ADDR_MASK_4K) as *mut u64;
    let _ = i2;
    pd
}

/// Maps one 2MB-aligned huge page.
unsafe fn map_2mb(pool: &mut TablePool, pml4_phys: u64, vaddr: u64, paddr: u64) {
    let pd = walk_to_pd(pool, pml4_phys, vaddr);
    let (_, _, i2) = indices(vaddr);
    *pd.add(i2) = (paddr & ADDR_MASK_2MB) | PAGE_PRESENT | PAGE_WRITABLE | PAGE_HUGE;
}

/// Maps one 4K page (used only for the small higher-half kernel range,
/// which isn't guaranteed 2MB-aligned).
unsafe fn map_4k(pool: &mut TablePool, pml4_phys: u64, vaddr: u64, paddr: u64) {
    let (i4, i3, i2, i1) = indices_4k(vaddr);
    let pml4 = pml4_phys as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        *pml4.add(i4) = pool.next() | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pdpt = (*pml4.add(i4) & ADDR_MASK_4K) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        *pdpt.add(i3) = pool.next() | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pd = (*pdpt.add(i3) & ADDR_MASK_4K) as *mut u64;
    if *pd.add(i2) & PAGE_PRESENT == 0 {
        *pd.add(i2) = pool.next() | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pt = (*pd.add(i2) & ADDR_MASK_4K) as *mut u64;
    *pt.add(i1) = (paddr & ADDR_MASK_4K) | PAGE_PRESENT | PAGE_WRITABLE;
}

/// Builds the bootstrap PML4: identity-maps every region the UEFI memory
/// map reports (2MB granularity, rounded outward — matches the old C
/// kernel/paging.c's "map every descriptor" approach, but explicitly
/// temporary here) plus the kernel's higher-half range at 4K granularity.
/// Returns the new PML4's physical address; does NOT install it — the
/// caller switches CR3 once this returns successfully, after confirming
/// there's nothing left to fail on.
pub unsafe fn build(
    pool: &mut TablePool,
    memory_map: *const EfiMemoryDescriptor,
    memory_map_size: u64,
    descriptor_size: u64,
    kernel_paddr_start: u64,
    kernel_paddr_end: u64,
    kernel_vaddr_start: u64,
) -> u64 {
    let pml4_phys = pool.next();

    let entries = memory_map_size / descriptor_size;
    for i in 0..entries {
        let desc = &*((memory_map as *const u8).add((i * descriptor_size) as usize)
            as *const EfiMemoryDescriptor);
        if desc.number_of_pages == 0 {
            continue;
        }
        let start = desc.physical_start;
        let len = desc.number_of_pages * 4096;
        let end = start + len;

        // Round outward to 2MB boundaries so a region that isn't
        // 2MB-aligned is still fully covered (over-mapping a little extra
        // identity range is harmless here — this table is temporary).
        let start_aligned = start & !0x1FFFFF;
        let end_aligned = (end + 0x1FFFFF) & !0x1FFFFF;

        let mut addr = start_aligned;
        while addr < end_aligned {
            map_2mb(pool, pml4_phys, addr, addr);
            addr += 0x200000;
        }
    }

    // Higher-half kernel range, 4K granularity (the kernel's own vmm::init
    // will replace this almost immediately with correctly-permissioned
    // per-segment mappings — this just has to be valid long enough to
    // execute the jump and reach that point).
    let offset = kernel_vaddr_start - kernel_paddr_start;
    let mut paddr = kernel_paddr_start & !0xFFF;
    let end = (kernel_paddr_end + 0xFFF) & !0xFFF;
    while paddr < end {
        map_4k(pool, pml4_phys, paddr + offset, paddr);
        paddr += 0x1000;
    }

    pml4_phys
}
