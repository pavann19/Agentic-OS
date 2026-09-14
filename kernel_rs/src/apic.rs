//! Local APIC timer. Nothing like this existed in the C kernel at all
//! (`docs/ROADMAP.md` Phase 0 item: "Local APIC timer as the periodic
//! tick, replacing PIC-only interrupt handling"). This is a from-scratch
//! addition, not a port.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::klog_info;

const IA32_APIC_BASE_MSR: u32 = 0x1B;
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000;

// Register offsets within the LAPIC's 4K MMIO page.
const REG_ID: u64 = 0x20;
const REG_SPURIOUS: u64 = 0xF0;
const REG_EOI: u64 = 0xB0;
const REG_ICR_LOW: u64 = 0x300;
const REG_ICR_HIGH: u64 = 0x310;
const REG_LVT_TIMER: u64 = 0x320;
const REG_TIMER_INITIAL_COUNT: u64 = 0x380;
const REG_TIMER_DIVIDE: u64 = 0x3E0;

// ICR (Interrupt Command Register) delivery-mode encodings (Intel SDM
// vol 3A 10.6.1) -- the two Phase 9 needs for real AP bring-up.
const ICR_DELIVERY_INIT: u32 = 0b101 << 8;
const ICR_DELIVERY_STARTUP: u32 = 0b110 << 8;
const ICR_LEVEL_ASSERT: u32 = 1 << 14;
const ICR_DELIVERY_STATUS_PENDING: u32 = 1 << 12;

pub const TIMER_VECTOR: u8 = 0x20;
const SPURIOUS_VECTOR: u8 = 0xFF;
const TIMER_PERIODIC: u32 = 1 << 17;
const APIC_SOFTWARE_ENABLE: u32 = 1 << 8;

static mut LAPIC_VADDR: u64 = 0;
static TICKS: AtomicU64 = AtomicU64::new(0);

unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    ((high as u64) << 32) | (low as u64)
}

unsafe fn read_reg(offset: u64) -> u32 {
    core::ptr::read_volatile((LAPIC_VADDR + offset) as *const u32)
}

unsafe fn write_reg(offset: u64, value: u32) {
    core::ptr::write_volatile((LAPIC_VADDR + offset) as *mut u32, value);
}

pub fn eoi() {
    unsafe { write_reg(REG_EOI, 0) };
}

/// This CORE's own local APIC ID -- real xAPIC hardware behavior, not a
/// software-assigned index: every core's LAPIC lives at the SAME
/// physical MMIO address (architectural, not a QEMU quirk), but each
/// core's own hardware answers with ITS OWN ID at that address, so
/// calling this from an AP (once its own paging/CR3 is live and it can
/// reach `LAPIC_VADDR`, already mapped read-only-in-effect by the BSP's
/// own `init()` and reachable through the SAME shared kernel page
/// tables every core uses) returns that AP's real ID, not the BSP's.
pub fn lapic_id() -> u32 {
    unsafe { (read_reg(REG_ID) >> 24) & 0xFF }
}

/// Phase 9 deliverable 2: whether this core's own LAPIC MMIO mapping is
/// live yet. `smp::current_cpu_index()` guards on this before ever
/// calling `lapic_id()` — a handful of early-boot call sites (BSP-only,
/// before `apic::init()` has run) touch per-CPU state before any LAPIC
/// is reachable, and reading MMIO through a zeroed `LAPIC_VADDR` would
/// be an immediate page fault, not a graceful fallback.
pub fn is_initialized() -> bool {
    unsafe { LAPIC_VADDR != 0 }
}

fn icr_wait_idle() {
    // Bounded, not infinite -- a stuck/never-idle ICR must never hang
    // the BSP forever (same "prove it timed out, don't just spin"
    // discipline as every other bounded poll in this codebase).
    let mut spins: u64 = 0;
    while unsafe { read_reg(REG_ICR_LOW) } & ICR_DELIVERY_STATUS_PENDING != 0 {
        spins += 1;
        if spins > 50_000_000 {
            klog_info!("APIC: ICR did not go idle -- proceeding anyway (bounded wait exhausted)");
            return;
        }
        core::hint::spin_loop();
    }
}

fn send_ipi(target_apic_id: u32, icr_low: u32) {
    unsafe {
        write_reg(REG_ICR_HIGH, target_apic_id << 24);
        write_reg(REG_ICR_LOW, icr_low);
    }
    icr_wait_idle();
}

/// Real INIT IPI (Intel MP/ACPI spec's own bring-up sequence, step 1) --
/// resets the target AP into a wait-for-SIPI state. `target_apic_id` is
/// a REAL MADT-reported APIC ID (`kernel_common::madt::CpuEntry`),
/// never a software index.
pub fn send_init(target_apic_id: u32) {
    send_ipi(target_apic_id, ICR_DELIVERY_INIT | ICR_LEVEL_ASSERT);
}

/// Real Startup IPI (SIPI) -- `vector` is the PAGE NUMBER (physical
/// address >> 12) the target AP starts executing real-mode code at,
/// CS:IP = vector<<8 : 0x0000 (Intel SDM 10.6.4). Sent TWICE per the
/// spec's own standard sequence (some real silicon needs the second
/// one; QEMU's own emulation tolerates it as a harmless no-op if the
/// first already succeeded) -- the caller (`smp::bring_up_all`) is
/// responsible for that, not this function.
pub fn send_sipi(target_apic_id: u32, vector: u8) {
    send_ipi(target_apic_id, ICR_DELIVERY_STARTUP | (vector as u32));
}

// Fixed-delivery-mode encoding (0b000, the ICR default) -- a real,
// ordinary interrupt at `vector`, exactly like any device IRQ, just
// targeted at a specific core instead of routed by the I/O APIC. Phase
// 9 deliverables 3 (reschedule IPI) and 4 (TLB shootdown IPI) both need
// exactly this: "deliver a real interrupt to that OTHER core, right now."
const ICR_DELIVERY_FIXED: u32 = 0b000 << 8;

/// Sends a real, ordinary fixed-vector IPI to `target_apic_id` --
/// `vector` must have a real handler installed in the (shared) IDT
/// before this is ever called, same requirement as any other interrupt
/// vector. Used by `smp::send_reschedule_ipi`/`smp::send_tlb_shootdown`.
pub fn send_ipi_vector(target_apic_id: u32, vector: u8) {
    send_ipi(target_apic_id, ICR_DELIVERY_FIXED | (vector as u32));
}

/// A bounded, uncalibrated busy-wait -- real time calibration (against
/// the PIT or a TSC-deadline reference) is real future work, same
/// honestly-stated gap `apic::init`'s own doc already carries for the
/// timer's initial count. Good enough for spacing real INIT/SIPI/SIPI
/// sends apart under QEMU TCG, which is the only target this function
/// is exercised against so far.
pub fn busy_wait(iterations: u64) {
    for _ in 0..iterations {
        core::hint::spin_loop();
    }
}

pub fn tick_count() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Called from interrupt context (`idt.rs::h_timer`). Does the minimum
/// possible: bump the counter, push a deferred event, EOI. No logging, no
/// formatting, nothing that could take unbounded time — that's the whole
/// point of `events.rs` existing.
pub fn on_tick() {
    let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::events::push(crate::events::Event::Tick(n));
    eoi();
}

/// Brings up the Local APIC and starts a periodic timer at `TIMER_VECTOR`
/// — BSP-only. Must run after `idt::init()` (the vector needs a handler
/// installed before unmasking it) and after `vmm::init()` (needs
/// `map_mmio_page`).
///
/// Phase 9: split into a one-time part (this function) and
/// `arm_timer_this_core` (below), because they are genuinely different
/// kinds of state. `LAPIC_VADDR`'s underlying MMIO PHYSICAL address is
/// architecturally the SAME on every real core (`lapic_id()`'s own doc
/// comment: "every core's LAPIC lives at the SAME physical MMIO
/// address... but each core's own hardware answers"), and the virtual
/// mapping for it lives in the one shared kernel PML4 every core
/// already uses — so the MMIO mapping itself only needs to happen ONCE,
/// by the BSP. But `REG_SPURIOUS`/`REG_LVT_TIMER`/`REG_TIMER_DIVIDE`/
/// `REG_TIMER_INITIAL_COUNT`, even though accessed through that SAME
/// shared virtual address, are genuinely PER-CORE hardware registers
/// (xAPIC's real architectural behavior: the identical address routes
/// to each core's own on-die LAPIC) — an AP that never writes them of
/// its own never gets a running local timer, and Phase 9 deliverable 3
/// needs every core to have one (it's what drives that core's own
/// `schedule()` calls).
pub fn init() {
    unsafe {
        let base = rdmsr(IA32_APIC_BASE_MSR);
        let phys = base & APIC_BASE_ADDR_MASK;
        // Ensure the enable bit is set (it should already be, on every
        // real/QEMU boot path, but this is cheap insurance rather than an
        // assumption).
        if base & APIC_BASE_ENABLE == 0 {
            let (low, high): (u32, u32);
            core::arch::asm!("rdmsr", in("ecx") IA32_APIC_BASE_MSR, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
            let new_low = low | (APIC_BASE_ENABLE as u32);
            core::arch::asm!("wrmsr", in("ecx") IA32_APIC_BASE_MSR, in("eax") new_low, in("edx") high, options(nomem, nostack, preserves_flags));
        }

        LAPIC_VADDR = crate::vmm::map_mmio_page(phys);
    }
    arm_timer_this_core();
    klog_info!("Local APIC timer started, vector=0x{:x}, periodic", TIMER_VECTOR);
}

/// Arms THIS CORE's own LAPIC timer -- called once by `init()` above
/// for the BSP, and directly by every AP (`smp.rs::ap_entry`, after
/// `LAPIC_VADDR` is already valid via the BSP's one-time mapping) for
/// itself. See `init`'s own doc comment for why this genuinely must run
/// per-core, not once.
pub fn arm_timer_this_core() {
    unsafe {
        write_reg(REG_SPURIOUS, (SPURIOUS_VECTOR as u32) | APIC_SOFTWARE_ENABLE);
        write_reg(REG_TIMER_DIVIDE, 0x3); // divide by 16
        write_reg(REG_LVT_TIMER, (TIMER_VECTOR as u32) | TIMER_PERIODIC);
        // Real, evidence-backed latency fix: reduced from 10_000_000 to
        // 1_000_000 (divide-by-16 divisor unchanged) to cut the preemption
        // quantum from ~150ms to ~15ms. With 6-8 threads in the round-robin
        // run queue the old value caused focused-window keystrokes to wait up
        // to ~1 second for a scheduling turn; 1_000_000 keeps the queue
        // drained within one ~15ms slot. No calibration against a real time
        // source yet — that remains real, disclosed future work.
        write_reg(REG_TIMER_INITIAL_COUNT, 1_000_000);
    }
}
