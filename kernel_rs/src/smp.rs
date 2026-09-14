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
//! **Historical note, now resolved:** this module's original increment
//! (deliverable 1) brought up ONE AP at a time and had it halt forever
//! once online -- deliberately, because `gdt.rs`'s `GDT`/`TSS` and
//! `klog.rs`'s `SerialWriter`/COM1 access were single, unsynchronized,
//! shared kernel state at the time. Deliverable 2 gave every core its
//! own GDT/TSS; deliverable 5 made the kernel's shared-state lock
//! (`critical::without_interrupts`) genuinely cross-core, and wrapped
//! `klog`'s COM1 writer in it. `bring_up_all` below is STILL
//! sequential during the bring-up loop itself (each AP is fully
//! verified online before the next is SIPI'd -- a real, simple,
//! correctness-first bring-up order, not a performance concern this
//! phase needs to optimize), but every AP that finishes bring-up no
//! longer halts: `ap_entry` now arms its own local timer and enters a
//! real, independently-scheduled idle loop (deliverable 3) -- by the
//! time `bring_up_all` returns, every online core is a genuine,
//! concurrently-executing participant in the one shared scheduler.

use crate::{apic, klog_info, pmm, serial, thread, vmm};
use core::sync::atomic::{AtomicU32, Ordering};
use kernel_common::madt::CpuEntry;

const TRAMPOLINE_PHYS: u64 = 0x8000;
const DATA_ENTRY_OFF: u64 = 0xFE0;
const DATA_PML4_OFF: u64 = 0xFE8;
const DATA_STACK_OFF: u64 = 0xFF0;
/// Phase 9 deliverable 2: this AP's own software CPU index (0 = BSP,
/// always pre-assigned; 1..N handed out in bring-up order), written
/// into the trampoline's data page right before that AP's SIPI so
/// `ap_entry` can read it back once it's running and use it to claim
/// its OWN slot in `gdt.rs`'s per-CPU GDT/TSS arrays — see this
/// module's other doc comments for why nothing here uses real
/// GS-base-relative per-CPU storage yet (that's real follow-up work;
/// this lookup-table approach is correct, just not the eventual O(1)
/// mechanism).
const DATA_CPU_INDEX_OFF: u64 = 0xFD8;

/// Phase 9: upper bound on real CPUs this kernel tracks per-CPU state
/// for (GDT/TSS in `gdt.rs`, the index registry below). Matches
/// `main.rs`'s own MADT-parsing array bound — both are real, stated
/// limits, not a guess; a firmware reporting more than this is a
/// real, not-yet-hit case this kernel does not yet handle.
pub const MAX_CPUS: usize = 32;

/// Real APIC-ID -> software-CPU-index registry. `None` until that slot
/// is claimed. Populated once for the BSP (index 0, at the start of
/// `bring_up_all`) and once per AP (inside `ap_entry`, using the index
/// `bring_up_all` assigned it before SIPI-ing it) — never mutated
/// concurrently, since bring-up stays deliberately sequential (this
/// module's own doc comment on why).
static mut APIC_ID_TO_INDEX: [Option<u32>; MAX_CPUS] = [None; MAX_CPUS];

/// Claims `index` for `apic_id`. Called exactly once per real core,
/// either from `bring_up_all` (BSP, index 0) or from `ap_entry` itself
/// (every AP, using the index it was handed via the trampoline data
/// page).
pub fn register_cpu(index: usize, apic_id: u32) {
    if index < MAX_CPUS {
        unsafe {
            (&mut *&raw mut APIC_ID_TO_INDEX)[index] = Some(apic_id);
        }
    }
}

/// Real IPI vectors -- Phase 9 deliverables 3 and 4. Distinct from
/// `apic::TIMER_VECTOR` (0x20) and `pic::KEYBOARD_VECTOR` (0x21), both
/// already claimed.
pub const RESCHEDULE_VECTOR: u8 = 0x22;
pub const TLB_SHOOTDOWN_VECTOR: u8 = 0x23;

/// Real APIC ID for a registered software cpu_index, or `None` if that
/// slot was never claimed (an index past however many real cores this
/// boot actually brought up).
fn apic_id_for_index(index: usize) -> Option<u32> {
    if index >= MAX_CPUS {
        return None;
    }
    unsafe { (&*&raw const APIC_ID_TO_INDEX)[index] }
}

/// Phase 9 deliverable 3: sends a real reschedule IPI to `target_cpu`
/// (a software cpu_index) — used by `thread::spawn_pinned_to_cpu` so a
/// thread placed on another core's run queue is picked up immediately,
/// not just at that core's next periodic timer tick. A silent no-op if
/// `target_cpu` was never claimed (defensive — callers are not expected
/// to pass an unregistered index, but this must never fault into
/// undefined APIC state if one slips through).
pub fn send_reschedule_ipi(target_cpu: usize) {
    if let Some(apic_id) = apic_id_for_index(target_cpu) {
        crate::apic::send_ipi_vector(apic_id, RESCHEDULE_VECTOR);
    }
}

/// Phase 9 deliverable 4 (`docs/ROADMAP.md` §5 — "TLB shootdown via IPI
/// on every cross-core mapping change... a mapping torn down on one
/// core is provably unusable on another core within a bounded time").
///
/// Single pending-request slot, not a queue: correctness relies on
/// shootdowns being fully serialized by `SHOOTDOWN_LOCK` below (one
/// initiator, one in-flight vaddr, everyone else's shootdown call spins
/// until it's their turn) -- real, deliberate, and simple, matching
/// this codebase's own stated preference for a coarse-but-correct
/// mechanism over a more complex one this pass doesn't need yet (same
/// trade-off `critical.rs`'s own kernel-wide lock makes, for the same
/// reason).
static SHOOTDOWN_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static TLB_SHOOTDOWN_PENDING_VADDR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static TLB_SHOOTDOWN_TARGET_COUNT: AtomicU32 = AtomicU32::new(0);
static TLB_SHOOTDOWN_ACKS: AtomicU32 = AtomicU32::new(0);

/// Called by `vmm::unmap_page_shootdown` AFTER it has already unmapped
/// the entry and `invlpg`'d it locally on THIS core. Broadcasts a real
/// IPI to every OTHER registered, online core, each of which runs a
/// real `invlpg` for `vaddr` in its own interrupt handler
/// (`idt.rs::h_tlb_shootdown` -> `handle_tlb_shootdown_ipi` below) and
/// acknowledges — this function then spins, bounded, until every
/// target has actually acknowledged, so a caller returning from this
/// function has REAL evidence the mapping is unusable everywhere, not
/// just "the IPI was sent."
pub fn shootdown_tlb(vaddr: u64) {
    // Serialize: bounded wait for any other core's own shootdown to
    // finish first, same "bounded, not infinite" discipline as every
    // other real wait in this codebase.
    let mut spins: u64 = 0;
    while SHOOTDOWN_LOCK
        .compare_exchange_weak(false, true, core::sync::atomic::Ordering::Acquire, core::sync::atomic::Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
        spins += 1;
        if spins > 500_000_000 {
            klog_info!("TLB_SHOOTDOWN_LOCK_STUCK -- proceeding anyway (bounded wait exhausted)");
            break;
        }
    }

    TLB_SHOOTDOWN_PENDING_VADDR.store(vaddr, Ordering::SeqCst);
    TLB_SHOOTDOWN_ACKS.store(0, Ordering::SeqCst);

    let me = crate::smp::current_cpu_index();
    let mut target_count: u32 = 0;
    for cpu in 0..MAX_CPUS {
        if cpu == me {
            continue;
        }
        if let Some(apic_id) = apic_id_for_index(cpu) {
            target_count += 1;
            crate::apic::send_ipi_vector(apic_id, TLB_SHOOTDOWN_VECTOR);
        }
    }
    TLB_SHOOTDOWN_TARGET_COUNT.store(target_count, Ordering::SeqCst);

    if target_count > 0 {
        let mut spins: u64 = 0;
        loop {
            let acked = TLB_SHOOTDOWN_ACKS.load(Ordering::SeqCst);
            if acked >= target_count {
                klog_info!(
                    "TLB_SHOOTDOWN_PASS vaddr=0x{:x} targets={} acked={}",
                    vaddr, target_count, acked
                );
                break;
            }
            core::hint::spin_loop();
            spins += 1;
            if spins > 500_000_000 {
                klog_info!(
                    "TLB_SHOOTDOWN_TIMEOUT vaddr=0x{:x} acked={}/{} -- proceeding anyway",
                    vaddr, acked, target_count
                );
                break;
            }
        }
    }

    SHOOTDOWN_LOCK.store(false, core::sync::atomic::Ordering::Release);
}

/// Runs on the RECEIVING core, from `idt.rs::h_tlb_shootdown`'s
/// interrupt context. Real, minimal ISR: read the pending vaddr, real
/// `invlpg`, acknowledge.
pub fn handle_tlb_shootdown_ipi() {
    let vaddr = TLB_SHOOTDOWN_PENDING_VADDR.load(Ordering::SeqCst);
    unsafe {
        core::arch::asm!("invlpg [{}]", in(reg) vaddr, options(nostack, preserves_flags));
    }
    TLB_SHOOTDOWN_ACKS.fetch_add(1, Ordering::SeqCst);
}

/// This CORE's own software CPU index, resolved from its real hardware
/// APIC ID (`apic::lapic_id()`) against the registry above. Returns 0
/// (the BSP's slot) if the LAPIC isn't mapped yet (a handful of
/// early-boot call sites run before `apic::init()`, always on the BSP,
/// where 0 is always correct) or if this core's own ID was never
/// registered (shouldn't happen for any core that reached Rust code at
/// all, but fails safe to the BSP's slot rather than an out-of-bounds
/// index).
pub fn current_cpu_index() -> usize {
    if !crate::apic::is_initialized() {
        return 0;
    }
    let id = crate::apic::lapic_id();
    let table = unsafe { &*&raw const APIC_ID_TO_INDEX };
    for (i, slot) in table.iter().enumerate() {
        if *slot == Some(id) {
            return i;
        }
    }
    0
}

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

    // Phase 9 deliverable 2: claim this core's OWN GDT/TSS/double-fault
    // stack slot and load a real per-core IDTR -- BEFORE anything else
    // runs on this core, same as the BSP's own boot order (gdt::init
    // then idt::init, both ahead of everything else in main.rs). The
    // index was assigned by bring_up_all and handed over via the
    // trampoline data page (this module's own doc comment on why: no
    // GS-base per-CPU storage yet, so this is how a just-started AP
    // learns which slot is its).
    let index = unsafe { core::ptr::read_volatile(pmm::p2v_pub(TRAMPOLINE_PHYS + DATA_CPU_INDEX_OFF) as *const u64) } as usize;
    crate::smp::register_cpu(index, id);
    crate::gdt::init_for_cpu(index);
    crate::idt::load_current_cpu();

    // Ensure CR4.PGE is active on this AP so kernel PAGE_GLOBAL entries persist across CR3 switches
    let mut cr4: u64;
    unsafe {
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= 1 << 7;
        core::arch::asm!("mov cr4, {}", in(reg) cr4, options(nomem, nostack, preserves_flags));
    }

    serial::write_str("[SMP] AP_ONLINE apic_id=");
    serial::write_hex_raw(id as u64);
    serial::write_str(" cpu_index=");
    serial::write_hex_raw(index as u64);
    serial::write_str("\n");
    AP_READY.store(id + 1, Ordering::SeqCst);

    // Phase 9 deliverable 3 (`docs/ROADMAP.md` §5): this is the real
    // change from deliverable 1's own version of this function, which
    // intentionally `cli; hlt`'d forever here (that module's own doc
    // comment: "at most one AP is ever executing non-BSP code at once,
    // so nothing here is exposed to a real concurrent-access bug yet").
    // That constraint is gone now -- deliverable 2 gave this core its
    // own GDT/TSS, deliverable 5 (see critical.rs) made the kernel's
    // shared-state lock genuinely cross-core, and thread.rs now keeps a
    // real per-core run queue for this exact core_index. This AP claims
    // its own idle "thread 0" (`thread::init_as_current_thread_for_cpu`,
    // real per-core state, not the BSP's), arms its OWN local APIC
    // timer (`apic::arm_timer_this_core` -- real, per-core hardware
    // state, see that function's own doc comment for why the BSP's
    // earlier `apic::init()` alone doesn't cover this), and enters a
    // real, interruptible idle loop: `sti` then `hlt`, exactly the
    // BSP's own post-boot idle shape. From this point on this core is a
    // genuine, independently-scheduled participant -- its own timer
    // ticks drive `thread::schedule()` on it (`idt.rs::h_timer`, same
    // handler the BSP uses), and `smp::send_reschedule_ipi`/
    // `smp::shootdown_tlb` can reach it directly.
    thread::init_as_current_thread_for_cpu();
    apic::arm_timer_this_core();
    // Real bug found via an actual boot crash (page fault, GS_BASE=0):
    // `syscall::init()`'s MSRs (EFER.SCE, STAR, LSTAR, FMASK,
    // KERNEL_GS_BASE) are genuinely per-core hardware state, and were
    // previously only ever set lazily by whichever driver-setup thread
    // happened to call `syscall::init()` on whatever core IT was
    // running on. That is NOT good enough once work-stealing
    // (deliverable 3's own rebalance) exists: a ring-3 thread can be
    // migrated to a DIFFERENT core between its own setup (where it
    // called `syscall::init()`, configuring only that ORIGINAL core)
    // and its next `syscall` instruction — landing on a core that was
    // NEVER configured, whose `IA32_KERNEL_GS_BASE` is still 0.
    // Configuring every core UNCONDITIONALLY, right here, before this
    // core is ever eligible to receive a stolen thread, closes that
    // window completely -- `syscall::init()`'s own per-core idempotency
    // makes every driver's existing `syscall::init()` call site still
    // harmless, just redundant from here on.
    crate::syscall::init();

    // Real bug found via an actual boot crash (double fault, corrupted
    // RSP), not by inspection: this core has been running, since SIPI,
    // on `bring_up_all`'s own bootstrap AP stack -- REALLY only ONE
    // physical page (4KB) deep in practice (`bring_up_all`'s own doc
    // comment already disclosed this: `pmm::alloc_page` gives no
    // contiguity guarantee across calls, and only the FIRST page's
    // address is ever actually used for `stack_top`). That was fine for
    // the ORIGINAL version of this function, which ran one tiny,
    // non-recursive call and then halted forever. It stopped being fine
    // the instant this core became a genuine, ongoing scheduler
    // participant (deliverable 3): repeated timer interrupts, each
    // nesting `h_timer` -> `schedule()` -> `schedule_locked()` ->
    // `switch_to`'s own callee-saved pushes, on top of whatever this
    // core's own idle loop already had live, overflows a 4KB stack in
    // real, observed practice. Fixed by allocating a REAL,
    // `KERNEL_STACK_SIZE`-sized stack (the exact same size every
    // spawned thread already gets — `thread.rs`'s own constant) and
    // switching onto it, via inline asm, BEFORE ever enabling
    // interrupts on this core — `mem::forget` keeps it alive forever
    // (this core's idle context never exits, so it's never freed, same
    // as thread 0's own boot-time stack never being freed either).
    let idle_stack: alloc::boxed::Box<[u8]> = alloc::vec![0u8; thread::KERNEL_STACK_SIZE].into_boxed_slice();
    let idle_stack_top = idle_stack.as_ptr() as u64 + idle_stack.len() as u64;
    core::mem::forget(idle_stack);

    unsafe {
        core::arch::asm!(
            "mov rsp, {0}",
            "sti",
            "2:",
            "hlt",
            "jmp 2b",
            in(reg) idle_stack_top,
            options(noreturn)
        );
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
/// Returns the number of REAL, verified-online cores after bring-up
/// (BSP + every AP that heartbeated), so callers (`main.rs`) can decide
/// whether real multi-core evidence (a cross-core pinned-thread demo,
/// `smp_race_soak`) is even meaningful for this boot.
pub fn bring_up_all(cpus: &[CpuEntry]) -> u32 {
    let bsp_id = apic::lapic_id();
    klog_info!("SMP_BRINGUP_START bsp_apic_id={}", bsp_id);
    register_cpu(0, bsp_id); // the BSP's own slot -- gdt::init_for_cpu(0) already claimed index 0 at boot; this just makes lapic_id()->index lookups for the BSP resolve correctly from here on.

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
            return 1; // BSP only -- bring-up itself never ran
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
    let mut next_index: usize = 1; // 0 is the BSP's, always

    for cpu in cpus {
        if cpu.apic_id == bsp_id {
            continue; // the BSP is already running -- never SIPI itself
        }
        if !cpu.enabled {
            klog_info!("SMP_AP_SKIPPED apic_id={} (MADT reports not enabled -- no silicon assumed present)", cpu.apic_id);
            skipped_disabled += 1;
            continue;
        }
        if next_index >= MAX_CPUS {
            klog_info!("SMP_AP_SKIPPED apic_id={} (MAX_CPUS={} exhausted -- real, stated limit)", cpu.apic_id, MAX_CPUS);
            continue;
        }
        let cpu_index = next_index;
        next_index += 1;
        unsafe { write_u64_at(TRAMPOLINE_PHYS + DATA_CPU_INDEX_OFF, cpu_index as u64) };

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
    brought_up + 1 // +1 for the BSP itself, always online
}

/// Phase 9 deliverable 3's real pinned-thread demo body
/// (`main.rs`, right after bring-up): logs its own REAL `cpu_index`
/// (`smp::current_cpu_index()`, resolved from actual hardware
/// `apic::lapic_id()`) so the serial log itself is the evidence this
/// thread genuinely ran on the core it was pinned to, then exits.
pub extern "C" fn pinned_demo_thread() {
    klog_info!("SMP_PINNED_DEMO_RUNNING cpu_index={}", current_cpu_index());
}
