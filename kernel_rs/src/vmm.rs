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
/// Dedicated MMIO window (`docs/ROADMAP.md`'s virtual layout) — separate
/// from the RAM direct-map window because MMIO physical addresses (e.g.
/// the Local APIC at 0xFEE00000) sit way outside the reported RAM span and
/// are never covered by that window's per-page loop.
pub const MMIO_VIRTUAL_BASE: u64 = 0xFFFF_FE00_0000_0000;

const PAGE_PRESENT: u64 = 1 << 0;
pub const PAGE_WRITABLE: u64 = 1 << 1;
pub const PAGE_CACHE_DISABLE: u64 = 1 << 4;
/// PAT bit for a 4KB leaf PTE (bit 7) — selects PAT entry 4 when PCD/PWT
/// are both 0 (the 3-bit PAT-table index is `PAT<<2 | PCD<<1 | PWT`).
/// See `enable_pat_write_combining`'s own doc for why entry 4
/// specifically, and why this is real, disclosed root-cause work for
/// the reported input-lag/"still slow" follow-up, not another guess.
const PAGE_PAT: u64 = 1 << 7;
/// Global bit for leaf PTE (bit 8). When CR4.PGE is enabled, translations for
/// pages with PAGE_GLOBAL set are not flushed on CR3 reload/context switch.
pub const PAGE_GLOBAL: u64 = 1 << 8;
pub const PAGE_NO_EXECUTE: u64 = 1 << 63;
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

// The real index-splitting math lives in kernel_common::pagetable,
// verified by host_tests/ against known-good vectors — this is a thin
// name-matching wrapper, not a parallel implementation.
fn indices(vaddr: u64) -> (usize, usize, usize, usize) {
    kernel_common::pagetable::split_indices(vaddr)
}

/// Debug-only: walks `pml4_phys`'s tables for `vaddr` and returns the
/// raw leaf PTE value (0 if not present at any level -- distinguishable
/// from a genuinely-zero-flags present PTE, which never happens since
/// PAGE_PRESENT is always set on any real mapping). Not used by normal
/// boot code; exists for exactly the kind of "does this address space
/// actually map what I think it maps" question real bug-hunting in this
/// kernel keeps needing.
pub unsafe fn debug_translate(pml4_phys: u64, vaddr: u64) -> u64 {
    let (i4, i3, i2, i1) = indices(vaddr);
    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        return 0;
    }
    let pdpt = pmm::p2v_pub(*pml4.add(i4) & ADDR_MASK) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        return 0;
    }
    let pd = pmm::p2v_pub(*pdpt.add(i3) & ADDR_MASK) as *mut u64;
    if *pd.add(i2) & PAGE_PRESENT == 0 {
        return 0;
    }
    let pt = pmm::p2v_pub(*pd.add(i2) & ADDR_MASK) as *mut u64;
    *pt.add(i1)
}

/// Real user-pointer validation — Phase 5's introspection syscall is the
/// first one in this kernel to actually dereference a caller-supplied
/// address (see `syscall.rs`'s long-standing "NO user-pointer validation
/// exists yet" note, open since Phase 1). Walks `pml4_phys` (the CALLING
/// process's own tables — pass `current_cr3()` from inside a syscall,
/// never a trusted/kernel PML4) page by page across `[vaddr, vaddr+len)`
/// and requires every page be PRESENT, WRITABLE, and USER-accessible.
/// Rejects on the first page that fails any of those, on overflow, and on
/// a zero-length range (nothing to write into is not a valid target). This
/// is what makes it safe for the kernel to write INTO a buffer a ring-3
/// caller named, rather than trusting the caller's own claim about what
/// it mapped.
pub unsafe fn validate_user_buffer_writable(pml4_phys: u64, vaddr: u64, len: u64) -> bool {
    if len == 0 {
        return false;
    }
    let Some(end) = vaddr.checked_add(len) else {
        return false;
    };
    let is_kernel = pml4_phys == KERNEL_PML4_PHYS;
    let required = if is_kernel {
        PAGE_PRESENT | PAGE_WRITABLE
    } else {
        PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER
    };
    let mut page = vaddr & !0xFFF;
    while page < end {
        let pte = debug_translate(pml4_phys, page);
        if pte & required != required {
            return false;
        }
        page += 0x1000;
    }
    true
}

/// Writes `data` into a already-`validate_user_buffer_writable`-checked
/// user range, one byte at a time via `write_volatile` — not a slice copy
/// (see `kernel_common::mem_intrinsics`'s module doc and this crate's own
/// `.cargo/config.toml`: any pattern LLVM can lower into a `memcpy` call
/// hits the same indirect-call toolchain bug that cost a full investigation
/// in Phase 4; a real, explicit, volatile per-byte loop is what reliably
/// avoids it here too). Physical translation is redone per byte rather
/// than cached across a page boundary — deliberately simple, since this
/// path only ever moves a few hundred bytes of small `#[repr(C)]` structs,
/// not a bulk data-transfer fast path.
pub unsafe fn write_user_bytes(pml4_phys: u64, vaddr: u64, data: &[u8]) {
    for (i, &byte) in data.iter().enumerate() {
        let addr = vaddr + i as u64;
        let (i4, i3, i2, i1) = indices(addr);
        let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
        let pdpt = pmm::p2v_pub(*pml4.add(i4) & ADDR_MASK) as *mut u64;
        let pd = pmm::p2v_pub(*pdpt.add(i3) & ADDR_MASK) as *mut u64;
        let pt = pmm::p2v_pub(*pd.add(i2) & ADDR_MASK) as *mut u64;
        let leaf_phys = (*pt.add(i1)) & ADDR_MASK;
        let dst = pmm::p2v_pub(leaf_phys + (addr & 0xFFF)) as *mut u8;
        core::ptr::write_volatile(dst, byte);
    }
}

/// Phase 12 deliverable 4 (minimal UI toolkit): the read-side sibling of
/// `validate_user_buffer_writable` above — required WRITABLE dropped
/// (reading text a process wants drawn doesn't need to write back to
/// it), PRESENT|USER kept. First real need for this direction: `SYS_
/// SURFACE_DRAW_TEXT` reads the caller's own request struct AND its
/// text bytes directly out of the CALLING process's address space
/// (safe specifically because a syscall handler runs under the
/// CALLER's own CR3 — unchanged by `SYSCALL`/`SYSRET` — so a validated
/// user vaddr is a real, dereferenceable pointer here, not a foreign
/// address needing translation).
pub unsafe fn validate_user_buffer_readable(pml4_phys: u64, vaddr: u64, len: u64) -> bool {
    if len == 0 {
        return false;
    }
    let Some(end) = vaddr.checked_add(len) else {
        return false;
    };
    let is_kernel = pml4_phys == KERNEL_PML4_PHYS;
    let required = if is_kernel {
        PAGE_PRESENT
    } else {
        PAGE_PRESENT | PAGE_USER
    };
    let mut page = vaddr & !0xFFF;
    while page < end {
        let pte = debug_translate(pml4_phys, page);
        if pte & required != required {
            return false;
        }
        page += 0x1000;
    }
    true
}

/// Reads `len` bytes out of an already-`validate_user_buffer_readable`-
/// checked user range, one byte at a time via `read_volatile` — same
/// explicit-loop discipline `write_user_bytes` already documents (never
/// a slice copy, to avoid this toolchain's known indirect-`memcpy`-call
/// bug).
pub unsafe fn read_user_bytes(pml4_phys: u64, vaddr: u64, out: &mut [u8]) {
    for (i, slot) in out.iter_mut().enumerate() {
        let addr = vaddr + i as u64;
        let (i4, i3, i2, i1) = indices(addr);
        let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
        let pdpt = pmm::p2v_pub(*pml4.add(i4) & ADDR_MASK) as *mut u64;
        let pd = pmm::p2v_pub(*pdpt.add(i3) & ADDR_MASK) as *mut u64;
        let pt = pmm::p2v_pub(*pd.add(i2) & ADDR_MASK) as *mut u64;
        let leaf_phys = (*pt.add(i1)) & ADDR_MASK;
        let src = pmm::p2v_pub(leaf_phys + (addr & 0xFFF)) as *const u8;
        *slot = core::ptr::read_volatile(src);
    }
}

/// Maps one 4K page. `flags` should be `PAGE_WRITABLE`/`PAGE_NO_EXECUTE` as
/// needed — `PAGE_PRESENT` is always added. Intermediate tables are
/// allocated on demand via the PMM (so this must only be called while the
/// PMM is initialized and, for any call after the CR3 switch, while
/// `pmm::PHYS_OFFSET` correctly resolves physical addresses — see pmm.rs).
unsafe fn map_page(pml4_phys: u64, vaddr: u64, paddr: u64, flags: u64) {
    let (i4, i3, i2, i1) = indices(vaddr);

    // Real bug this session found: intermediate PML4E/PDPTE/PDE entries
    // were created with only PAGE_PRESENT|PAGE_WRITABLE — never
    // PAGE_USER. Per x86_64 paging rules, EVERY level of the translation
    // needs the USER bit set for a CPL3 access to succeed at all,
    // regardless of what the LEAF PTE allows; missing it at any
    // intermediate level makes the whole path supervisor-only. Confirmed
    // by testing: a leaf PTE correctly marked PAGE_USER still produced a
    // page fault with the instruction-fetch/present bits set (error code
    // 0x15) the instant ring-3 code tried to execute from it, because the
    // PML4E/PDPTE/PDE covering that address had no USER bit. Standard
    // practice (matching Linux and others) is to set USER permissively on
    // intermediate tables and let the LEAF's own R/W/X bits be the real
    // per-page gate — this doesn't grant broader access than intended,
    // since every level still needs PRESENT and the leaf still needs its
    // own correct flags.
    let intermediate_flags = PAGE_PRESENT | PAGE_WRITABLE | PAGE_USER;

    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        let new = zeroed_table();
        *pml4.add(i4) = new | intermediate_flags;
    }
    let pdpt_phys = *pml4.add(i4) & ADDR_MASK;

    let pdpt = pmm::p2v_pub(pdpt_phys) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        let new = zeroed_table();
        *pdpt.add(i3) = new | intermediate_flags;
    }
    let pd_phys = *pdpt.add(i3) & ADDR_MASK;

    let pd = pmm::p2v_pub(pd_phys) as *mut u64;
    if *pd.add(i2) & PAGE_PRESENT == 0 {
        let new = zeroed_table();
        *pd.add(i2) = new | intermediate_flags;
    }
    let pt_phys = *pd.add(i2) & ADDR_MASK;

    let pt = pmm::p2v_pub(pt_phys) as *mut u64;
    *pt.add(i1) = (paddr & ADDR_MASK) | flags | PAGE_PRESENT;
}

/// Translates a virtual address to its underlying physical address by walking `pml4_phys`.
/// Returns `None` if any level of the translation is missing / not present.
pub unsafe fn virt_to_phys(pml4_phys: u64, vaddr: u64) -> Option<u64> {
    let (i4, i3, i2, i1) = indices(vaddr);
    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;
    if *pml4.add(i4) & PAGE_PRESENT == 0 {
        return None;
    }
    let pdpt_phys = *pml4.add(i4) & ADDR_MASK;
    let pdpt = pmm::p2v_pub(pdpt_phys) as *mut u64;
    if *pdpt.add(i3) & PAGE_PRESENT == 0 {
        return None;
    }
    let pd_phys = *pdpt.add(i3) & ADDR_MASK;
    let pd = pmm::p2v_pub(pd_phys) as *mut u64;
    if *pd.add(i2) & PAGE_PRESENT == 0 {
        return None;
    }
    let pt_phys = *pd.add(i2) & ADDR_MASK;
    let pt = pmm::p2v_pub(pt_phys) as *mut u64;
    let pte = *pt.add(i1);
    if pte & PAGE_PRESENT == 0 {
        return None;
    }
    Some((pte & ADDR_MASK) | (vaddr & 0xFFF))
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

/// Phase 9 deliverable 4 (`docs/ROADMAP.md` §5 — "TLB shootdown via IPI
/// on every cross-core mapping change — a correctness requirement, not
/// an optimization, given Phase 0's per-page permission model"): same
/// as `unmap_page`, but ALSO broadcasts a real cross-core shootdown
/// (`smp::shootdown_tlb`) after the local unmap+`invlpg` completes, so
/// a caller returning from this function has real, waited-for evidence
/// the mapping is gone from every online core's TLB, not just this
/// one's. Any unmap whose vaddr might be reachable through more than
/// this one core's own translation (a shared kernel mapping, or a
/// process address space that could genuinely be active on another
/// core) MUST go through this, not the plain `unmap_page` above — a
/// stale TLB entry surviving on another core is exactly the kind of
/// "revoked but still usable" gap Phase 2's whole capability-revocation
/// guarantee depends on not existing.
pub unsafe fn unmap_page_shootdown(pml4_phys: u64, vaddr: u64) {
    unsafe {
        unmap_page(pml4_phys, vaddr);
    }
    crate::smp::shootdown_tlb(vaddr);
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
        let mut flags = PAGE_NO_EXECUTE | PAGE_GLOBAL;
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
            PAGE_WRITABLE | PAGE_NO_EXECUTE | PAGE_GLOBAL,
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
            PAGE_WRITABLE | PAGE_NO_EXECUTE | PAGE_GLOBAL,
        );
        off += pmm::PAGE_SIZE;
    }

    // Enable Page Global Enable (PGE) in CR4 (bit 7) so kernel mappings with
    // PAGE_GLOBAL remain resident in the TLB across CR3 reloads / context switches.
    let mut cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    cr4 |= 1 << 7;
    core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));

    klog_info!("VMM: kernel + direct-map window + stack region built (PAGE_GLOBAL enabled), switching CR3");

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

/// Creates a new, independent address space for a process: a fresh PML4
/// whose upper half (indices 256-511 — the canonical-high range, exactly
/// where x86_64's sign-extension boundary sits, not an arbitrary choice)
/// is copied from the kernel's own PML4, and whose lower half (0-255, user
/// space) starts completely empty. Copying only the PML4 ENTRIES (not the
/// tables they point to) is what makes kernel mappings shared rather than
/// duplicated — every process's PML4[256..512] points at the SAME
/// PDPT/PD/PT structures the kernel itself uses, so kernel code/heap/
/// direct-map/MMIO stay reachable from ring 0 no matter which process's
/// CR3 is loaded, while user-space mappings (built by the caller into the
/// returned PML4's lower half) are completely independent per process.
/// Returns the new PML4's physical address.
pub unsafe fn new_address_space() -> u64 {
    let new_pml4_phys = pmm::alloc_page(); // already zeroed by pmm::alloc_page
    let new_pml4 = pmm::p2v_pub(new_pml4_phys) as *mut u64;
    let kernel_pml4 = pmm::p2v_pub(KERNEL_PML4_PHYS) as *mut u64;
    for i in 256..512 {
        *new_pml4.add(i) = *kernel_pml4.add(i);
    }
    new_pml4_phys
}

/// Destroys a process address space: traverses all user-space mappings (indices 0..256
/// of the PML4), freeing all leaf user physical pages and all intermediate page table
/// frames (PT, PD, PDPT) back to the PMM allocator, then frees the PML4 frame itself.
///
/// Ensures KERNEL_PML4_PHYS and page 0 are never freed. If CR3 currently points to
/// `pml4_phys`, switches to `KERNEL_PML4_PHYS` first to prevent executing/translating
/// with a freed page hierarchy.
pub unsafe fn destroy_address_space(pml4_phys: u64) {
    if pml4_phys == 0 || pml4_phys == KERNEL_PML4_PHYS {
        return;
    }

    // Safety: if the current core is running on this address space, switch back to the kernel PML4 first.
    if current_cr3() == pml4_phys {
        switch_address_space(KERNEL_PML4_PHYS);
    }

    let pml4 = pmm::p2v_pub(pml4_phys) as *mut u64;

    // Traverse the lower half (user-space: entries 0..256).
    // Entries 256..512 are shared kernel structures and must NEVER be freed!
    for i4 in 0..256 {
        let pml4e = *pml4.add(i4);
        if pml4e & PAGE_PRESENT != 0 {
            let pdpt_phys = pml4e & ADDR_MASK;
            let pdpt = pmm::p2v_pub(pdpt_phys) as *mut u64;

            for i3 in 0..512 {
                let pdpte = *pdpt.add(i3);
                if pdpte & PAGE_PRESENT != 0 {
                    let pd_phys = pdpte & ADDR_MASK;
                    let pd = pmm::p2v_pub(pd_phys) as *mut u64;

                    for i2 in 0..512 {
                        let pde = *pd.add(i2);
                        if pde & PAGE_PRESENT != 0 {
                            let pt_phys = pde & ADDR_MASK;
                            let pt = pmm::p2v_pub(pt_phys) as *mut u64;

                            for i1 in 0..512 {
                                let pte = *pt.add(i1);
                                if pte & PAGE_PRESENT != 0 {
                                    let leaf_phys = pte & ADDR_MASK;
                                    pmm::free_page(leaf_phys);
                                }
                            }
                            pmm::free_page(pt_phys);
                        }
                    }
                    pmm::free_page(pd_phys);
                }
            }
            pmm::free_page(pdpt_phys);
        }
    }

    // Free the PML4 table itself.
    pmm::free_page(pml4_phys);
}

/// Maps one page into `pml4_phys`'s address space at `vaddr` -> `paddr`
/// with the given flags — the general-purpose version of `map_page` for
/// callers outside this module (process/user-space setup). `flags` should
/// NOT include `PAGE_PRESENT` (added automatically); pass `PAGE_USER` for
/// any mapping a ring-3 process needs to access itself.
pub unsafe fn map_page_in(pml4_phys: u64, vaddr: u64, paddr: u64, flags: u64) {
    map_page(pml4_phys, vaddr, paddr, flags);
}

pub const PAGE_USER: u64 = 1 << 2;

/// Switches CR3 to `pml4_phys` — a full TLB flush (every non-global page).
/// Callers (the scheduler) should avoid calling this when the incoming
/// thread's address space is already the one currently loaded.
pub unsafe fn switch_address_space(pml4_phys: u64) {
    core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));
}

/// Reads the currently-loaded CR3 (physical PML4 address, low 12 bits
/// masked off since CR3 carries PCID/flags there we don't use yet).
pub fn current_cr3() -> u64 {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
    }
    cr3 & 0x000F_FFFF_FFFF_F000
}

/// Maps one page into the kernel heap window (`heap.rs`'s
/// `HEAP_VIRTUAL_BASE`), RW+NX — heap memory is data, never code. Uses the
/// kernel's OWN production tables (post-CR3-switch), not the bootstrap
/// ones — callable only after `vmm::init()` has run.
pub unsafe fn map_heap_page(vaddr: u64, paddr: u64) {
    map_page(KERNEL_PML4_PHYS, vaddr, paddr, PAGE_WRITABLE | PAGE_NO_EXECUTE | PAGE_GLOBAL);
}

/// Real, disclosed fast-path fix for the input-typing-lag report: every
/// caller of `map_mmio_page` (framebuffer pixel writes, one call per
/// pixel) used to walk/insert into the real page tables via `map_page`
/// on EVERY SINGLE pixel, even though consecutive pixels in a redraw
/// overwhelmingly fall in the SAME already-mapped 4KB page (the
/// framebuffer's own linear layout puts ~1024 32bpp pixels per page).
/// `map_page` is idempotent, so re-calling it for an already-mapped
/// page was always correct, just needlessly slow -- a full 4-level walk
/// per pixel for a single on-screen text redraw (thousands of pixels)
/// is real, measurable latency between a keystroke and its echo. Fixed
/// by remembering the single most-recently-mapped page's physical
/// address and skipping the walk entirely when the next pixel's page is
/// the same one -- correct because mappings here are permanent (never
/// unmapped/moved once established), so a cache hit can never be stale.
static LAST_MMIO_PAGE_PADDR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Maps one MMIO page (e.g. the Local APIC) into the MMIO window at
/// `MMIO_VIRTUAL_BASE + paddr`, RW+NX+cache-disabled — MMIO registers must
/// never be cached, or writes/reads can silently hit a stale cache line
/// instead of the real device. Returns the mapped virtual address.
pub unsafe fn map_mmio_page(paddr: u64) -> u64 {
    let page_paddr = paddr & !0xFFF;
    let vaddr = MMIO_VIRTUAL_BASE + page_paddr;
    if LAST_MMIO_PAGE_PADDR.load(core::sync::atomic::Ordering::Relaxed) != page_paddr {
        map_page(
            KERNEL_PML4_PHYS,
            vaddr,
            page_paddr,
            PAGE_WRITABLE | PAGE_NO_EXECUTE | PAGE_CACHE_DISABLE | PAGE_GLOBAL,
        );
        LAST_MMIO_PAGE_PADDR.store(page_paddr, core::sync::atomic::Ordering::Relaxed);
    }
    vaddr + (paddr & 0xFFF)
}

/// Real root cause behind the "typing still feels slow" report after
/// the page-walk-cache and batched-present fixes: real `rdtsc`
/// measurement (see `terminal_emulator`'s own diagnostic) showed
/// `draws` (writes into a window's own RAM buffer) costing ~1-2M
/// cycles per keystroke while `present` (the same number of pixels,
/// but through `map_mmio_page`'s UC/`PAGE_CACHE_DISABLE` mapping into
/// the real framebuffer) cost ~18-22M cycles -- roughly 300 real CPU
/// cycles per single 4-byte pixel store, an order of magnitude more
/// than the walk-cache fix alone could explain. Root cause: `UC`
/// (strong, fully uncached) memory is the CORRECT type for a real
/// device's control/status registers (LAPIC, IOMMU, etc. — ordering
/// there matters, and those still use `map_mmio_page` unchanged), but
/// it is real, unnecessary overkill for a linear framebuffer, where
/// real hardware and every real OS instead use Write-Combining (WC):
/// stores can be buffered/coalesced and are only made visible in bulk,
/// which is what actually makes bulk pixel writes fast. Fixed by
/// reprogramming PAT entry 4 (via `enable_pat_write_combining`, called
/// once at boot) from its reset default (WB) to WC, and mapping the
/// framebuffer with the PAT bit set (selecting entry 4) instead of
/// `PAGE_CACHE_DISABLE` (which selects entry 2, UC-) -- a dedicated
/// function, not a change to `map_mmio_page` itself, so every other
/// real MMIO device in this kernel keeps its correct, unmodified UC
/// mapping.
static LAST_FB_PAGE_PADDR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(u64::MAX);

/// Real, one-time PAT setup — must run once at boot, before any real
/// framebuffer page is mapped via `map_framebuffer_page` (main.rs calls
/// this immediately after `vmm::init()`, well before `compositor_demo`/
/// `terminal_demo` ever touch a framebuffer). Reprograms ONLY PAT entry
/// 4 (WB -> WC); entries 0-3, 5-7 keep the architectural power-on reset
/// values (Intel SDM Vol.3A, PAT MSR reset value) unchanged, so every
/// existing mapping in this kernel (all of which select entry 0 or 2,
/// never 4) is completely unaffected — this is additive, not a global
/// cache-policy change.
pub unsafe fn enable_pat_write_combining() {
    const IA32_PAT: u32 = 0x277;
    // PA0=WB(06) PA1=WT(04) PA2=UC-(07) PA3=UC(00) [reset defaults,
    // unchanged] PA4=WC(01) [changed from reset default WB(06)]
    // PA5=WT(04) PA6=UC-(07) PA7=UC(00) [reset defaults, unchanged]
    let pat_value: u64 =
        0x06 | (0x04 << 8) | (0x07 << 16) | (0x00 << 24) | (0x01 << 32) | (0x04 << 40) | (0x07 << 48) | (0x00 << 56);
    let lo = pat_value as u32;
    let hi = (pat_value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") IA32_PAT,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack)
    );
    crate::klog_info!("VMM_PAT_WRITE_COMBINING_ENABLED entry4=WC");
}

/// Maps one framebuffer page with Write-Combining instead of strict UC
/// (see this section's own doc for why) — same last-page-cache
/// discipline as `map_mmio_page`, kept as its OWN cache (not shared)
/// since the two functions map different, unrelated physical regions.
pub unsafe fn map_framebuffer_page(paddr: u64) -> u64 {
    let page_paddr = paddr & !0xFFF;
    let vaddr = MMIO_VIRTUAL_BASE + page_paddr;
    if LAST_FB_PAGE_PADDR.load(core::sync::atomic::Ordering::Relaxed) != page_paddr {
        map_page(KERNEL_PML4_PHYS, vaddr, page_paddr, PAGE_WRITABLE | PAGE_NO_EXECUTE | PAGE_PAT | PAGE_GLOBAL);
        LAST_FB_PAGE_PADDR.store(page_paddr, core::sync::atomic::Ordering::Relaxed);
    }
    vaddr + (paddr & 0xFFF)
}

/// Bulk Write-Combining framebuffer range mapping — called ONCE at boot,
/// before any window content is ever presented. Maps every page in the
/// framebuffer's physical span [paddr, paddr+size) into the kernel's
/// dedicated MMIO virtual window with PAT=WC. After this call:
///
///  1. `map_framebuffer_page` finds its LAST_FB_PAGE_PADDR cache warm on
///     every call (no page-table walk, just an atomic load and branch),
///     cutting the per-pixel overhead from a full 4-level walk to ~zero.
///
///  2. The stable `MMIO_VIRTUAL_BASE + page_paddr` addresses are contiguous
///     for an entire scanline, letting `present_partial`'s per-scanline
///     loop be replaced with a single `copy_nonoverlapping` (triggering
///     CPU WC buffer coalescing) instead of QUEUE_DEPTH scalar stores.
///
/// Real, disclosed cost: only correct because the framebuffer's physical
/// address is fixed at boot (UEFI GOP) and never moves. A driver that
/// DMA-remaps its framebuffer at runtime would need to call this again
/// after each remap — not applicable here.
pub unsafe fn map_framebuffer_range(paddr: u64, size: u64) {
    let start_page = paddr & !0xFFF;
    let end_page = ((paddr + size).wrapping_add(0xFFF)) & !0xFFF;
    let mut p = start_page;
    while p < end_page {
        let vaddr = MMIO_VIRTUAL_BASE + p;
        map_page(KERNEL_PML4_PHYS, vaddr, p, PAGE_WRITABLE | PAGE_NO_EXECUTE | PAGE_PAT | PAGE_GLOBAL);
        p += 0x1000;
    }
    // Warm the single-page cache to the first page so subsequent
    // map_framebuffer_page calls never need to re-insert it.
    LAST_FB_PAGE_PADDR.store(start_page, core::sync::atomic::Ordering::Relaxed);
    crate::klog_info!(
        "VMM_FB_RANGE_MAPPED paddr=0x{:x} size={} pages={}",
        paddr,
        size,
        (end_page - start_page) / 0x1000
    );
}

