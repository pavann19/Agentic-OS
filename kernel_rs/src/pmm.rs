//! Physical memory manager. Port of `kernel/memory.c`'s two-bitmap design
//! (used + reserved, tracked separately so a permanently-reserved page and
//! a merely-in-use page can't be confused — see the original repo audit,
//! which called this out as the strongest-evidenced C subsystem). Ported
//! rather than redesigned from scratch: the design itself was sound, only
//! the language changes.

#![allow(dead_code)]

use crate::bootinfo::{BootInfo, EfiMemoryDescriptor};
use crate::klog_info;

pub const PAGE_SIZE: u64 = 4096;
const EFI_CONVENTIONAL_MEMORY: u32 = 7;
const EFI_LOADER_CODE: u32 = 1;
const EFI_LOADER_DATA: u32 = 2;
const EFI_BOOT_SERVICES_CODE: u32 = 3;
const EFI_BOOT_SERVICES_DATA: u32 = 4;
const EFI_RUNTIME_SERVICES_CODE: u32 = 5;
const EFI_RUNTIME_SERVICES_DATA: u32 = 6;
const EFI_ACPI_RECLAIM_MEMORY: u32 = 9;
const EFI_ACPI_MEMORY_NVS: u32 = 10;

/// Descriptor types that represent real, addressable memory the PMM should
/// span. An ALLOWLIST, not a denylist — this is the fix for a real bug
/// this session found: the naive "highest address across every descriptor"
/// scan (what `kernel/memory.c` did in C, ported here at first) picked up
/// an `EfiReservedMemoryType` descriptor at 0xfd00000000 (a ~12GB QEMU q35
/// PCIe reservation window, confirmed via a diagnostic scan — NOT the MMIO
/// type this was first guessed to be), inflating the computed span to
/// ~1TB and, downstream, making vmm::init() try to direct-map ~268 million
/// pages and instantly exhaust real RAM. An allowlist of the types that
/// actually participate in normal allocation (loader/boot-services/
/// runtime-services/conventional/ACPI-reclaim/ACPI-NVS) is more robust
/// against whatever else firmware decides to report as "reserved" than
/// trying to enumerate every excludable type by name.
/// `docs/ROADMAP.md`'s own Memory Policy calls this class of bug out
/// explicitly ("usable pages come only from firmware descriptors that are
/// truly usable") — this is that fix, not a port of the original bug.
fn is_real_memory(ty: u32) -> bool {
    matches!(
        ty,
        EFI_LOADER_CODE
            | EFI_LOADER_DATA
            | EFI_BOOT_SERVICES_CODE
            | EFI_BOOT_SERVICES_DATA
            | EFI_RUNTIME_SERVICES_CODE
            | EFI_RUNTIME_SERVICES_DATA
            | EFI_CONVENTIONAL_MEMORY
            | EFI_ACPI_RECLAIM_MEMORY
            | EFI_ACPI_MEMORY_NVS
    )
}

// Physical-address-to-pointer translation offset. 0 during the bootstrap
// phase (identity mapping is still active from the bootloader's temporary
// tables — physical == virtual, so `addr as *mut u8` is directly valid).
// vmm::init() sets this to PHYS_MAP_BASE once the kernel's OWN page tables
// are live and the temporary identity map is gone — from that point on,
// a raw physical address is NOT a valid pointer on its own, it must go
// through the direct-map window. Every PMM pointer dereference in this file
// goes through `p2v()` specifically so this one flip is the only place that
// has to know both phases exist.
static mut PHYS_OFFSET: u64 = 0;

#[inline(always)]
unsafe fn p2v(paddr: u64) -> *mut u8 {
    (paddr + PHYS_OFFSET) as *mut u8
}

/// Same translation, exposed for vmm.rs's page-table walk (which needs to
/// dereference physical table addresses read out of page-table entries).
#[inline(always)]
pub unsafe fn p2v_pub(paddr: u64) -> *mut u8 {
    p2v(paddr)
}

/// Called exactly once by `vmm::init()` right after the CR3 switch to the
/// kernel's production tables. Must never be called before that switch
/// (there is nothing at PHYS_MAP_BASE yet) or more than once.
pub unsafe fn set_phys_offset(offset: u64) {
    PHYS_OFFSET = offset;
}

static mut USED_BITMAP: u64 = 0; // physical address; translated via p2v() on use
static mut RESERVED_BITMAP: u64 = 0;
static mut TOTAL_SPAN_PAGES: u64 = 0;
static mut LAST_SCANNED_PAGE: u64 = 0;

#[derive(Clone, Copy, Default)]
pub struct PmmStats {
    pub total_physical_span: u64,
    pub total_usable_memory: u64,
    pub reserved_memory: u64,
    pub free_pages: u64,
    pub used_pages: u64,
}

static mut STATS: PmmStats = PmmStats {
    total_physical_span: 0,
    total_usable_memory: 0,
    reserved_memory: 0,
    free_pages: 0,
    used_pages: 0,
};

pub fn stats() -> PmmStats {
    unsafe { STATS }
}

// Length used to build a slice view over a bitmap for kernel_common's pure
// functions to operate on — set once in init(). Both bitmaps are the same
// size (see init()), so one length covers either.
static mut BITMAP_LEN_BYTES: u64 = 0;

unsafe fn bitmap_slice_mut(bitmap_phys: u64) -> &'static mut [u8] {
    core::slice::from_raw_parts_mut(p2v(bitmap_phys), BITMAP_LEN_BYTES as usize)
}

unsafe fn bitmap_slice(bitmap_phys: u64) -> &'static [u8] {
    core::slice::from_raw_parts(p2v(bitmap_phys), BITMAP_LEN_BYTES as usize)
}

unsafe fn bitmap_set(bitmap_phys: u64, index: u64) {
    kernel_common::bitmap::set(bitmap_slice_mut(bitmap_phys), index);
}

unsafe fn bitmap_clear(bitmap_phys: u64, index: u64) {
    kernel_common::bitmap::clear(bitmap_slice_mut(bitmap_phys), index);
}

unsafe fn bitmap_test(bitmap_phys: u64, index: u64) -> bool {
    kernel_common::bitmap::test(bitmap_slice(bitmap_phys), index)
}

/// Marks `[start, start+length)` reserved AND used, idempotently. Matches
/// `pmm_reserve_range` exactly, including its "round the covering page range
/// outward, even across an unaligned start" behavior — the actual rounding
/// math now lives in kernel_common::bitmap::reserve_range_pages, verified
/// by host_tests/ against known-good vectors rather than only by booting.
pub unsafe fn reserve_range(start: u64, length: u64, reason: &str) {
    let (start_page, pages) = kernel_common::bitmap::reserve_range_pages(start, length, PAGE_SIZE);

    for p in 0..pages {
        let idx = start_page + p;
        if idx >= TOTAL_SPAN_PAGES {
            continue;
        }
        if !bitmap_test(RESERVED_BITMAP, idx) {
            bitmap_set(RESERVED_BITMAP, idx);
            if !bitmap_test(USED_BITMAP, idx) {
                bitmap_set(USED_BITMAP, idx);
                STATS.free_pages -= 1;
            }
            STATS.reserved_memory += PAGE_SIZE;
        }
    }
    klog_info!("Reserved {} pages for {} (start: 0x{:x})", pages, reason, start);
}

pub unsafe fn dump_stats() {
    // Copy out to a local first (matches `stats()`'s own approach) rather
    // than holding several `&STATS.field` shared references to the mutable
    // static across the whole function — avoids the static_mut_refs lint
    // for a real reason (those references living across further mutation
    // elsewhere would be genuinely unsound), not just to silence it.
    let s = STATS;
    klog_info!("--- PMM Stats ---");
    klog_info!("Total Span: {} MB", s.total_physical_span / (1024 * 1024));
    klog_info!("Total Usable: {} MB", s.total_usable_memory / (1024 * 1024));
    klog_info!("Reserved: {} KB", s.reserved_memory / 1024);
    klog_info!("Free Pages: {}", s.free_pages);
    klog_info!("Used Pages: {}", s.used_pages);
}

/// Scans the UEFI memory map to find total physical span, places two
/// bitmaps in a large-enough EfiConventionalMemory region, marks everything
/// used-by-default then frees conventional memory, then explicitly reserves
/// page zero, the bitmaps themselves, the kernel image, BootInfo, and the
/// memory map buffer. Framebuffer/font reservation is deferred to when
/// graphics is ported (kernel_rs doesn't touch those pointers yet).
pub unsafe fn init(boot_info: &BootInfo) {
    let map = boot_info.payload.memory_map;
    let entries = boot_info.payload.memory_map_size / boot_info.payload.memory_map_descriptor_size;
    let desc_size = boot_info.payload.memory_map_descriptor_size;

    let desc_at = |i: u64| -> &'static EfiMemoryDescriptor {
        &*((map as *const u8).add((i * desc_size) as usize) as *const EfiMemoryDescriptor)
    };

    let mut highest_address: u64 = 0;
    for i in 0..entries {
        let d = desc_at(i);
        let end = d.physical_start + d.number_of_pages * PAGE_SIZE;
        if !is_real_memory(d.ty) {
            continue;
        }
        if end > highest_address {
            highest_address = end;
        }
    }

    TOTAL_SPAN_PAGES = highest_address / PAGE_SIZE;
    STATS.total_physical_span = highest_address;

    let mut bitmap_size_bytes = TOTAL_SPAN_PAGES / 8;
    if TOTAL_SPAN_PAGES % 8 != 0 {
        bitmap_size_bytes += 1;
    }
    let total_bitmap_size = bitmap_size_bytes * 2;

    // A real bug this session found (present conceptually in the C
    // original too, just never triggered there): using physical address 0
    // as the "not found" sentinel is wrong when a legitimate
    // EfiConventionalMemory region legitimately starts AT address 0 (which
    // it does in this exact environment — conv[0] starts at 0x0) — a
    // successful match was being treated as failure. Fixed with an
    // explicit found-flag. Also require `physical_start > 0` specifically
    // (not just "found"): placing the bitmaps' actual bytes on page 0
    // would work today (reserve_range(0, ...) below marks it reserved
    // regardless of what's there), but keeping page 0 genuinely unused —
    // not merely marked-reserved-after-the-fact — is the more defensible
    // reading of "null guard," and it costs nothing here since larger
    // regions exist.
    let mut used_bitmap_addr: u64 = 0;
    let mut found = false;
    for i in 0..entries {
        let d = desc_at(i);
        if d.ty == EFI_CONVENTIONAL_MEMORY
            && d.physical_start > 0
            && d.number_of_pages * PAGE_SIZE >= total_bitmap_size
        {
            used_bitmap_addr = d.physical_start;
            found = true;
            break;
        }
    }
    if !found {
        crate::klog::panic("PMM: no free contiguous region for bitmaps");
    }

    USED_BITMAP = used_bitmap_addr;
    RESERVED_BITMAP = used_bitmap_addr + bitmap_size_bytes;
    BITMAP_LEN_BYTES = bitmap_size_bytes;

    for i in 0..bitmap_size_bytes {
        *p2v(USED_BITMAP).add(i as usize) = 0xFF;
        *p2v(RESERVED_BITMAP).add(i as usize) = 0x00;
    }

    STATS.used_pages = 0;
    STATS.free_pages = 0;
    STATS.total_usable_memory = 0;
    STATS.reserved_memory = 0;

    for i in 0..entries {
        let d = desc_at(i);
        if d.ty == EFI_CONVENTIONAL_MEMORY {
            STATS.total_usable_memory += d.number_of_pages * PAGE_SIZE;
            let start_page = d.physical_start / PAGE_SIZE;
            for p in 0..d.number_of_pages {
                bitmap_clear(USED_BITMAP, start_page + p);
                STATS.free_pages += 1;
            }
        }
    }

    reserve_range(0, PAGE_SIZE, "Page Zero");
    reserve_range(used_bitmap_addr, total_bitmap_size, "PMM Bitmaps");
    reserve_range(
        boot_info.payload.kernel_physical_start,
        boot_info.payload.kernel_physical_end - boot_info.payload.kernel_physical_start,
        "Kernel",
    );
    reserve_range(
        boot_info as *const BootInfo as u64,
        core::mem::size_of::<BootInfo>() as u64,
        "BootInfo",
    );
    reserve_range(
        map as u64,
        boot_info.payload.memory_map_size,
        "Memory Map Buffer",
    );

    dump_stats();
}

/// Allocates and zeroes one physical page. Returns 0 on exhaustion (matches
/// the C original's sentinel — callers must check).
///
/// Real bug found and fixed (see `critical.rs`'s doc comment for the full
/// investigation): the bitmap scan-then-mark below used to run with
/// interrupts enabled, letting a preemption between the scan and the
/// mark hand the SAME physical page to two different callers. Wrapped in
/// `without_interrupts` now — the whole scan+mark is atomic w.r.t. the
/// only thing that could ever interleave with it on a single core.
pub unsafe fn alloc_page() -> u64 {
    crate::critical::without_interrupts(|| unsafe { alloc_page_locked() })
}

unsafe fn alloc_page_locked() -> u64 {
    let mut i = LAST_SCANNED_PAGE;
    while i < TOTAL_SPAN_PAGES {
        if !bitmap_test(USED_BITMAP, i) {
            bitmap_set(USED_BITMAP, i);
            STATS.free_pages -= 1;
            STATS.used_pages += 1;
            LAST_SCANNED_PAGE = i;
            let addr = i * PAGE_SIZE;
            core::ptr::write_bytes(p2v(addr), 0, PAGE_SIZE as usize);
            return addr;
        }
        i += 1;
    }
    if LAST_SCANNED_PAGE > 0 {
        LAST_SCANNED_PAGE = 0;
        return alloc_page_locked();
    }
    klog_info!("OUT OF MEMORY: no physical pages left");
    0
}

/// Same real-bug fix as `alloc_page` — the clear-and-update sequence
/// below is now atomic w.r.t. preemption too, so a concurrent
/// `alloc_page` can never observe a half-freed page.
pub unsafe fn free_page(addr: u64) {
    crate::critical::without_interrupts(|| unsafe { free_page_locked(addr) })
}

unsafe fn free_page_locked(addr: u64) {
    if addr == 0 {
        klog_info!("pmm_free_page: reject null/page 0");
        return;
    }
    if addr % PAGE_SIZE != 0 {
        klog_info!("pmm_free_page: reject unaligned (0x{:x})", addr);
        return;
    }
    let page_index = addr / PAGE_SIZE;
    if page_index >= TOTAL_SPAN_PAGES {
        klog_info!("pmm_free_page: reject out-of-range (0x{:x})", addr);
        return;
    }
    if bitmap_test(RESERVED_BITMAP, page_index) {
        klog_info!("pmm_free_page: reject reserved (0x{:x})", addr);
        return;
    }
    if !bitmap_test(USED_BITMAP, page_index) {
        klog_info!("pmm_free_page: reject already free (0x{:x})", addr);
        return;
    }
    bitmap_clear(USED_BITMAP, page_index);
    STATS.free_pages += 1;
    STATS.used_pages -= 1;
    if page_index < LAST_SCANNED_PAGE {
        LAST_SCANNED_PAGE = page_index;
    }
}
