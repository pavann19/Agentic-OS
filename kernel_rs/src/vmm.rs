//! Virtual memory manager. Replaces `kernel/paging.c`'s design entirely
//! rather than porting it — the C version identity-mapped essentially all
//! physical RAM with `PAGE_READ_WRITE` on every entry, which
//! `docs/ROADMAP.md` explicitly calls out as needing replacement before
//! anything else builds on it (no permissions, no null-guard fault, no
//! unmap). This is that replacement.
//!
//! Virtual layout (`docs/ROADMAP.md` §"Virtual Memory Policy"):
//!   - Page 0: never mapped (null-guard).
//!   - `KERNEL_VIRTUAL_BASE` (0xFFFFFFFF80000000): higher-half kernel image.
//!     MUST match `kernel_rs/linker.ld`'s base and `boot_rs`'s bootstrap
//!     mapping — three independent places that have to agree, same
//!     discipline as the BootInfo ABI mirror.
//!   - `PHYS_MAP_BASE` (0xFFFF800000000000): direct-map window covering all
//!     physical RAM, offset-mapped (`vaddr = PHYS_MAP_BASE + paddr`). This
//!     is what replaces "treat a physical address as a pointer" once the
//!     temporary bootstrap identity map is gone — see `pmm.rs`'s
//!     `p2v()`/`set_phys_offset()`.
//!   - User region (below the canonical-address split): reserved, unused
//!     until Phase 1.
//!
//! The bootstrap-to-production transition: `boot_rs` builds a MINIMAL,
//! temporary table before jumping to the kernel — identity-mapping all
//! reported physical memory (so newly-PMM-allocated pages, wherever the
//! allocator finds them, are reachable while the real tables are being
//! built) plus the higher-half kernel mapping (so execution can continue
//! after the jump). `vmm::init()` here builds the REAL, permission-correct
//! tables from scratch and switches CR3 to them — the bootstrap identity
//! map is never carried forward; once this returns, only what's explicitly
//! mapped below is reachable.

#![allow(dead_code)]

use crate::bootinfo::BootInfo;
use crate::klog_info;
use crate::pmm;

pub const KERNEL_VIRTUAL_BASE: u64 = 0xFFFF_FFFF_8000_0000;
pub const PHYS_MAP_BASE: u64 = 0xFFFF_8000_0000_0000;

const PAGE_PRESENT: u64 = 1 << 0;
const PAGE_WRITABLE: u64 = 1 << 1;
const PAGE_NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

#[repr(C, align(4096))]
struct PageTable {
    entries: [u64; 512],
}

static mut KERNEL_PML4_PHYS: u64 = 0;

unsafe fn zeroed_table() -> u64 {
    // pmm::alloc_page() already zeroes the page (see pmm.rs) — no double
    // work needed, this just names the intent at each call site.
    let page = pmm::alloc_page();
    if page == 0 {
        // alloc_page()'s OOM sentinel is 0 — silently using that as a page
        // table's physical base would corrupt every entry built on top of
        // it (every subsequent write lands at physical address 0, i.e.
        // page zero, which is reserved and meant to stay untouched). A
        // page table exhausting the PMM this early is a real, fatal
        // condition worth a clear panic, not a corrupted boot that fails
        // mysteriously three calls later.
        crate::klog::panic("VMM: out of physical pages while building page tables");
    }
    page
}

fn indices(vaddr: u64) -> (usize, usize, usize, usize) {
    let pt = ((vaddr >> 12) & 0x1ff) as usize;
    let pd = ((vaddr >> 21) & 0x1ff) as usize;
    let pdpt = ((vaddr >> 30) & 0x1ff) as usize;
    let pml4 = ((vaddr >> 39) & 0x1ff) as usize;
    (pml4, pdpt, pd, pt)
}

/// Maps one 4K page. `flags` should be `PAGE_WRITABLE`/`PAGE_NO_EXECUTE` as
/// needed — `PAGE_PRESENT` is always added. Intermediate tables are
/// allocated on demand via the PMM (so this must only be called while the
/// PMM is initialized and, for any call after the CR3 switch, while
/// `pmm::PHYS_OFFSET` correctly resolves physical addresses — see pmm.rs).
unsafe fn map_page(pml4_phys: u64, vaddr: u64, paddr: u64, flags: u64) {
    let (i4, i3, i2, i1) = indices(vaddr);

    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        let new = zeroed_table();
        *pml4.add(i4) = new | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pdpt_phys = *pml4.add(i4) & ADDR_MASK;

    let pdpt = pmm::p2v_pub(pdpt_phys) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        let new = zeroed_table();
        *pdpt.add(i3) = new | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pd_phys = *pdpt.add(i3) & ADDR_MASK;

    let pd = pmm::p2v_pub(pd_phys) as *mut u64;
    if *pd.add(i2) & PAGE_PRESENT == 0 {
        let new = zeroed_table();
        *pd.add(i2) = new | PAGE_PRESENT | PAGE_WRITABLE;
    }
    let pt_phys = *pd.add(i2) & ADDR_MASK;

    let pt = pmm::p2v_pub(pt_phys) as *mut u64;
    *pt.add(i1) = (paddr & ADDR_MASK) | flags | PAGE_PRESENT;
}

/// Real unmap: clears the leaf PTE and invalidates the TLB entry. Does NOT
/// free now-empty intermediate tables (page-table reclamation is a real
/// future optimization, not a correctness requirement — an unmapped page
/// staying non-present is what matters). A vaddr with no mapping present at
/// any level is a silent no-op, matching typical unmap semantics.
pub unsafe fn unmap_page(pml4_phys: u64, vaddr: u64) {
    let (i4, i3, i2, i1) = indices(vaddr);
    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        return;
    }
    let pdpt = pmm::p2v_pub(*pml4.add(i4) & ADDR_MASK) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        return;
    }
    let pd = pmm::p2v_pub(*pdpt.add(i3) & ADDR_MASK) as *mut u64;
    if *pd.add(i2) & PAGE_PRESENT == 0 {
        return;
    }
    let pt = pmm::p2v_pub(*pd.add(i2) & ADDR_MASK) as *mut u64;
    *pt.add(i1) = 0;
    core::arch::asm!("invlpg [{}]", in(reg) vaddr, options(nostack, preserves_flags));
}

pub struct KernelSegment {
    pub vaddr: u64,
    pub paddr: u64,
    pub len: u64,
    pub writable: bool,
    pub executable: bool,
}

/// Builds the real, permission-correct kernel page tables and switches CR3
/// to them. Must be called after `pmm::init()`. `segments` describes the
/// kernel's own PT_LOAD ranges (so its code/data get correct, non-blanket
/// permissions instead of the RWX-everywhere the bootstrap map used) —
/// passed in from `main.rs`, which gets them from the linker via `extern`
/// symbols (see main.rs).
pub unsafe fn init(boot_info: &BootInfo, segments: &[KernelSegment], current_stack_ptr: u64) {
    let pml4_phys = zeroed_table();
    KERNEL_PML4_PHYS = pml4_phys;

    // Higher-half kernel image: map each real segment with its actual
    // permissions (never blanket RWX). Page 0 is never touched here or
    // anywhere else in this function — that omission IS the null-guard.
    for seg in segments {
        let mut flags = PAGE_NO_EXECUTE;
        if seg.writable {
            flags |= PAGE_WRITABLE;
        }
        if seg.executable {
            flags &= !PAGE_NO_EXECUTE;
        }
        let pages = (seg.len + pmm::PAGE_SIZE - 1) / pmm::PAGE_SIZE;
        for p in 0..pages {
            let off = p * pmm::PAGE_SIZE;
            map_page(pml4_phys, seg.vaddr + off, seg.paddr + off, flags);
        }
    }

    // Physical direct-map window: every physical page up to the reported
    // span, offset-mapped at PHYS_MAP_BASE, RW + NX (this is data access to
    // arbitrary physical memory, never code).
    let span_pages = pmm::stats().total_physical_span / pmm::PAGE_SIZE;
    let mut p = 0u64;
    while p < span_pages {
        let paddr = p * pmm::PAGE_SIZE;
        map_page(
            pml4_phys,
            PHYS_MAP_BASE + paddr,
            paddr,
            PAGE_WRITABLE | PAGE_NO_EXECUTE,
        );
        p += 1;
    }

    // Identity-map a 2MB-aligned region around the CURRENT stack. RSP was
    // set by UEFI/firmware before the bootloader even ran, pointing
    // somewhere in low memory that is neither the kernel's higher-half
    // range nor (at the correct offset) the direct-map window — without
    // this, the instruction immediately after the CR3 switch below faults
    // on its own stack access. Found by testing, not anticipated: the
    // first attempt at this switch produced no further serial output at
    // all past "switching CR3" (a silent triple-fault under QEMU, since no
    // exception handlers exist yet in kernel_rs to report one). Identity
    // (not direct-map-offset) specifically, so the stack pointer's VALUE
    // doesn't need to change at all — simplest fix that doesn't require
    // relocating a live stack out from under the currently-executing
    // function, which would also break this function's own ability to
    // return to its caller.
    // Maps THREE 2MB-aligned regions (6MB total: one below, the captured
    // point's own, one above) rather than just the one containing the
    // captured RSP. Found necessary by testing, not anticipated: RSP was
    // captured early in kernel_main, then the stack grew deeper across
    // several nested calls (gdt::init/idt::init/pmm::init, then vmm::init
    // itself, then core::fmt's formatting machinery) before the first
    // hex-formatted klog_info! call after the CR3 switch — which faulted,
    // while a bare serial::write_str immediately after the switch had not.
    // A single 2MB window anchored to the EARLY capture point apparently
    // didn't cover how much deeper the stack had grown by that point; wide
    // margin in both directions is simpler and more robust than computing
    // the exact depth precisely for a stack that's temporary anyway (a
    // real dedicated, guard-paged kernel stack is Phase 1 scope).
    let aligned = current_stack_ptr & !0x1F_FFFF;
    let stack_region_base = aligned - 0x20_0000;
    let mut off = 0u64;
    while off < 0x60_0000 {
        map_page(
            pml4_phys,
            stack_region_base + off,
            stack_region_base + off,
            PAGE_WRITABLE | PAGE_NO_EXECUTE,
        );
        off += pmm::PAGE_SIZE;
    }

    klog_info!("VMM: kernel + direct-map window + stack region built, switching CR3");

    core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));

    // Only NOW is the direct-map window live — flip PMM's translation from
    // identity (bootstrap) to PHYS_MAP_BASE-relative. Any PMM call before
    // this line in this function used identity addressing (still valid,
    // the bootstrap map was active); any PMM call after this line in the
    // rest of the kernel must go through this offset.
    pmm::set_phys_offset(PHYS_MAP_BASE);

    let _ = boot_info;
    klog_info!("VMM: CR3 switched, direct-map window active at 0x{:x}", PHYS_MAP_BASE);
}

pub fn kernel_pml4_phys() -> u64 {
    unsafe { KERNEL_PML4_PHYS }
}
