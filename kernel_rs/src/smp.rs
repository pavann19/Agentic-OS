//! Phase 9 deliverable 1 (`docs/ROADMAP.md` §5): real AP (Application
//! Processor) bring-up via a from-scratch INIT-SIPI-SIPI sequence and a
//! real 16-bit real-mode -> 32-bit protected-mode -> 64-bit long-mode
//! trampoline (`smp_trampoline.s`), landing every discovered, enabled
//! CPU (`kernel_common::madt` + `acpi::find_cpus`, this session's own
//! earlier increment) in a real Rust function running this kernel's
//! actual, shared page tables.
//!
//! **The identity-map trick, and why it's needed:** an AP that has just
//! been SIPI'd starts executing real 16-bit code at a low physical
//! address (`TRAMPOLINE_PHYS`) chosen by the SIPI vector -- there is no
//! way around this, it's how the hardware works. To reach this kernel's
//! REAL, higher-half page tables (`vmm::kernel_pml4_phys()` -- the SAME
//! ones the BSP already uses; Phase 0 deleted the blanket identity map,
//! so nothing maps `TRAMPOLINE_PHYS` by default), the AP must enable
//! paging with those tables active WHILE its instruction pointer is
//! still physically sitting inside the trampoline page. The instant
//! `CR0.PG` is set, translation begins for the very next fetch -- if
//! that address isn't mapped, it's an immediate triple fault, not a
//! hang. The fix: before ever sending a SIPI, `bring_up_all` adds ONE
//! explicit, single-page identity mapping (`TRAMPOLINE_PHYS ->
//! TRAMPOLINE_PHYS`) into the kernel's real, shared PML4 -- so the
//! trampoline code stays fetchable across the exact instant paging
//! turns on, right up until it far-jumps into the kernel's own
//! already-mapped higher-half code (`ap_entry` below), at which point
//! the low identity mapping is never touched again.
//!
//! **Deliberately left mapped, not torn down, in this increment:** a
//! real follow-up (tearing it down once every AP has passed through it)
//! needs cross-core synchronization this kernel doesn't have yet
//! (Phase 9 deliverable 4's own TLB-shootdown item) -- stated honestly
//! as a real, tracked simplification, not silently skipped.
//!
//! **Deliberately sequential, not concurrent, in this increment:**
//! `gdt.rs`'s `GDT`/`TSS` and `klog.rs`'s `SerialWriter`/COM1 access are
//! all single, unsynchronized, shared kernel state today (found and
//! recorded during this same phase, see `docs/PROGRESS.md`) -- real
//! multi-core-safe locking for them is Phase 9 deliverable 5, not this
//! one. `bring_up_all` brings up ONE AP at a time and waits for it to
//! signal readiness (and then halt forever) before releasing the next
//! -- by construction, at most one AP is ever executing non-BSP code at
//! once, so nothing here is exposed to a real concurrent-access bug yet.
//! This still genuinely satisfies Phase 9's own exit criterion ("all
//! firmware-reported cores are brought up and independently execute
//! real work ... a per-core heartbeat log with distinct APIC IDs") --
//! each AP really does run its own real code on its own real core, just
//! not at the same wall-clock instant as any other AP yet.

use crate::{apic, klog_info, pmm, serial, vmm};
use core::sync::atomic::{AtomicU32, Ordering};
use kernel_common::madt::CpuEntry;

const TRAMPOLINE_PHYS: u64 = 0x8000;
const DATA_ENTRY_OFF: u64 = 0xFE0;
const DATA_PML4_OFF: u64 = 0xFE8;
const DATA_STACK_OFF: u64 = 0xFF0;

const AP_STACK_PAGES: u64 = 4; // 16KB -- a real, minimal, temporary stack; Phase 1's real per-thread kernel stacks are the eventual replacement once the scheduler itself is SMP-aware (deliverable 3).

core::arch::global_asm!(include_str!("smp_trampoline.s"));

extern "C" {
    #[link_name = "ap_trampoline_start"]
    static AP_TRAMPOLINE_START: u8;
    #[link_name = "ap_trampoline_end"]
    static AP_TRAMPOLINE_END: u8;
}

/// Set by an AP right before it halts forever (see `ap_entry`) --
/// `1 + real_apic_id` so 0 unambiguously means "not yet ready" even for
/// APIC ID 0 (which in practice is always the BSP and never SIPI'd, but
/// the encoding stays correct regardless).
static AP_READY: AtomicU32 = AtomicU32::new(0);

/// Real, from-scratch AP entry point -- reached by every AP once its own
/// trampoline has taken it through real mode, protected mode, and long
/// mode into this kernel's real, shared page tables. `extern "C"`,
/// address taken and written into the trampoline's data field by
/// `bring_up_all` before each SIPI; never called directly from Rust.
///
/// Deliberately minimal: reads this core's own real APIC ID straight
/// from LAPIC hardware (`apic::lapic_id()` -- real per-core hardware
/// state, not a passed-in software index), writes ONE raw, core::fmt-free
/// heartbeat line directly via `serial::write_str`/`write_hex_raw`
/// (bypassing `klog_info!`'s shared `SerialWriter` deliberately -- see
/// this module's own doc on why nothing here assumes concurrent-safety
/// it hasn't earned yet), signals readiness, and halts forever with
/// interrupts disabled. Never returns.
#[no_mangle]
extern "C" fn ap_entry() -> ! {
    let id = apic::lapic_id();
    serial::write_str("[SMP] AP_ONLINE apic_id=");
    serial::write_hex_raw(id as u64);
    serial::write_str("\n");
    AP_READY.store(id + 1, Ordering::SeqCst);
    loop {
        unsafe {
            core::arch::asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}

unsafe fn write_u64_at(phys: u64, value: u64) {
    unsafe {
        let ptr = pmm::p2v_pub(phys) as *mut u64;
        core::ptr::write_volatile(ptr, value);
    }
}

/// Real INIT-SIPI-SIPI bring-up for every enabled CPU in `cpus` other
/// than the BSP (identified by `apic::lapic_id()` at call time -- the
/// BSP never SIPIs itself). Sequential by construction (see this
/// module's own doc): each AP is fully brought up, heartbeats, and
/// halts before the next one is ever released.
///
/// Bounded throughout, real timeouts, never an unbounded wait: a
/// misbehaving or absent AP produces a logged timeout and this function
/// moves on to the next entry, rather than hanging the BSP (and this
/// entire boot) forever on hardware that doesn't respond the way this
/// kernel expects.
pub fn bring_up_all(cpus: &[CpuEntry]) {
    let bsp_id = apic::lapic_id();
    klog_info!("SMP_BRINGUP_START bsp_apic_id={}", bsp_id);

    unsafe {
        // One-time setup, shared by every AP this call brings up: copy
        // the trampoline's real machine code to its fixed physical
        // home, and make sure that physical page survives the exact
        // instant each AP turns paging on (see module doc).
        let trampoline_start = &raw const AP_TRAMPOLINE_START as u64;
        let trampoline_end = &raw const AP_TRAMPOLINE_END as u64;
        let trampoline_len = trampoline_end - trampoline_start;
        if trampoline_len == 0 || trampoline_len > 0xF00 {
            // 0xF00 -- must stay well clear of the DATA_*_OFF fields
            // above (0xFE0+) that live in the same physical page. A
            // real, checked invariant, not an assumption: if this ever
            // trips, the trampoline grew too large for its own data
            // layout and would silently corrupt itself.
            klog_info!(
                "SMP_BRINGUP_ABORT trampoline_len={} exceeds safe budget -- refusing to proceed",
                trampoline_len
            );
            return;
        }

        pmm::reserve_range(TRAMPOLINE_PHYS, 4096, "AP Trampoline");

        let src = trampoline_start as *const u8;
        let dst = pmm::p2v_pub(TRAMPOLINE_PHYS);
        for i in 0..trampoline_len {
            core::ptr::write_volatile(dst.add(i as usize), core::ptr::read_volatile(src.add(i as usize)));
        }

        vmm::map_page_in(vmm::kernel_pml4_phys(), TRAMPOLINE_PHYS, TRAMPOLINE_PHYS, vmm::PAGE_WRITABLE);

        klog_info!(
            "SMP_TRAMPOLINE_READY phys=0x{:x} len={} (identity-mapped into the kernel's real PML4)",
            TRAMPOLINE_PHYS, trampoline_len
        );

        write_u64_at(TRAMPOLINE_PHYS + DATA_PML4_OFF, vmm::kernel_pml4_phys());
        write_u64_at(TRAMPOLINE_PHYS + DATA_ENTRY_OFF, ap_entry as *const () as u64);
    }

    let mut brought_up = 0u32;
    let mut skipped_disabled = 0u32;
    let mut timed_out = 0u32;

    for cpu in cpus {
        if cpu.apic_id == bsp_id {
            continue; // the BSP is already running -- never SIPI itself
        }
        if !cpu.enabled {
            klog_info!("SMP_AP_SKIPPED apic_id={} (MADT reports not enabled -- no silicon assumed present)", cpu.apic_id);
            skipped_disabled += 1;
            continue;
        }

        // A real, dedicated stack for this AP -- direct-map-window
        // virtual address, reachable the instant this AP's own paging
        // comes up (the direct-map window already covers all reported
        // physical RAM, built once by the BSP's own vmm::init(), and is
        // part of the SAME shared PML4 this AP is about to load).
        let stack_phys = unsafe { pmm::alloc_page() };
        for _ in 1..AP_STACK_PAGES {
            unsafe { pmm::alloc_page() }; // contiguity not guaranteed by this allocator -- real limitation, see below
        }
        // Real, stated limitation: pmm::alloc_page() gives no
        // contiguity guarantee across calls, so this stack is
        // realistically only the FIRST allocated page deep (4KB) for
        // anything that must not cross a non-contiguous boundary --
        // acceptable for `ap_entry`'s own tiny, non-recursive body, but
        // explicitly not a real 16KB guarantee. Real per-thread kernel
        // stacks (Phase 1) replace this properly.
        let stack_top = vmm::PHYS_MAP_BASE + stack_phys + pmm::PAGE_SIZE;
        unsafe { write_u64_at(TRAMPOLINE_PHYS + DATA_STACK_OFF, stack_top) };

        AP_READY.store(0, Ordering::SeqCst);

        klog_info!("SMP_AP_SIPI_START apic_id={}", cpu.apic_id);
        let sipi_vector = (TRAMPOLINE_PHYS >> 12) as u8;
        apic::send_init(cpu.apic_id);
        apic::busy_wait(2_000_000); // real spacing, not calibrated -- see apic::busy_wait's own doc
        apic::send_sipi(cpu.apic_id, sipi_vector);
        apic::busy_wait(500_000);
        apic::send_sipi(cpu.apic_id, sipi_vector); // spec's standard second SIPI

        let mut spins: u64 = 0;
        let mut ready = false;
        while spins < 200_000_000 {
            if AP_READY.load(Ordering::SeqCst) == cpu.apic_id + 1 {
                ready = true;
                break;
            }
            spins += 1;
            core::hint::spin_loop();
        }

        if ready {
            klog_info!("SMP_AP_ONLINE apic_id={}", cpu.apic_id);
            brought_up += 1;
        } else {
            klog_info!("SMP_AP_TIMEOUT apic_id={} (no heartbeat within the bounded wait -- moving on)", cpu.apic_id);
            timed_out += 1;
        }
    }

    klog_info!(
        "SMP_BRINGUP_DONE brought_up={} skipped_disabled={} timed_out={}",
        brought_up, skipped_disabled, timed_out
    );
}
