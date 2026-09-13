//! IDT covering all 32 CPU exception vectors — the C kernel only handled 3
//! of 32 (`kernel/interrupts.c`); `docs/ROADMAP.md` Phase 0 calls that out
//! explicitly as a requirement to fix. Uses Rust's native `x86-interrupt`
//! calling convention (`#![feature(abi_x86_interrupt)]` in main.rs) instead
//! of hand-written `__attribute__((interrupt))` trampolines — the compiler
//! generates the correct prologue/epilogue (iretq, stack alignment) for us,
//! which is exactly the class of hand-asm bug this feature exists to avoid.

#![allow(dead_code)]

use crate::gdt::DOUBLE_FAULT_IST_VALUE;
use crate::klog_error;

const IDT_ENTRIES: usize = 256;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

const NULL_ENTRY: IdtEntry = IdtEntry {
    offset_low: 0,
    selector: 0,
    ist: 0,
    type_attr: 0,
    offset_mid: 0,
    offset_high: 0,
    zero: 0,
};

#[repr(C, packed)]
struct IdtDescriptor {
    limit: u16,
    base: u64,
}

static mut IDT: [IdtEntry; IDT_ENTRIES] = [NULL_ENTRY; IDT_ENTRIES];

/// Matches what the CPU pushes for an `x86-interrupt`-ABI handler: RIP,
/// CS, RFLAGS, RSP, SS — the standard 5-field frame every reference on
/// `abi_x86_interrupt` describes.
#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

fn set_entry(vector: usize, handler: u64, ist: u8) {
    unsafe {
        IDT[vector].offset_low = (handler & 0xFFFF) as u16;
        IDT[vector].selector = 0x08; // kernel code segment, gdt.rs
        IDT[vector].ist = ist;
        IDT[vector].type_attr = 0x8E; // present, DPL0, 64-bit interrupt gate
        IDT[vector].offset_mid = ((handler >> 16) & 0xFFFF) as u16;
        IDT[vector].offset_high = ((handler >> 32) & 0xFFFFFFFF) as u32;
        IDT[vector].zero = 0;
    }
}

fn report(vector: u8, error_code: Option<u64>, frame: &InterruptStackFrame) {
    let rip = frame.instruction_pointer;
    let rsp = frame.stack_pointer;
    let cs = frame.code_segment;
    let flags = frame.cpu_flags;
    match error_code {
        Some(ec) => klog_error!(
            "EXCEPTION vector={} error_code=0x{:x} rip=0x{:x} cs=0x{:x} rflags=0x{:x} rsp=0x{:x}",
            vector, ec, rip, cs, flags, rsp
        ),
        None => klog_error!(
            "EXCEPTION vector={} rip=0x{:x} cs=0x{:x} rflags=0x{:x} rsp=0x{:x}",
            vector, rip, cs, flags, rsp
        ),
    }
}

fn read_cr2() -> u64 {
    let cr2: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags));
    }
    cr2
}

fn halt_forever() -> ! {
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}

/// Real Phase 1 exit-criterion closed here (see
/// `thread::kill_current_and_reschedule`'s doc comment for the full
/// story): a fault whose `InterruptStackFrame` shows CS's RPL was 3
/// (ring 3 — `frame.code_segment & 0x3 == 3`) now kills ONLY the
/// faulting process and lets the system continue, instead of halting the
/// whole kernel like every OTHER exception (and like this same fault
/// used to, before this fix) still does. A fault at CPL0 stays
/// unconditionally fatal — that's the kernel itself faulting, not a
/// process's own mistake, and there is no safe "just kill it" recovery
/// for that.
fn recover_or_halt(vector: u8, frame: &InterruptStackFrame) -> ! {
    if frame.code_segment & 0x3 == 3 {
        klog_error!("PROCESS_KILLED: fault occurred in ring 3 -- terminating this process, system continues");
        // Phase 9.5a: real observation point, BEFORE the thread is
        // actually killed (thread::current_id() must still resolve to
        // the dying thread here) -- if this was a registered driver
        // process, this drives a REAL crash-to-restart, not a
        // simulated one. A no-op for any other ring-3 thread.
        crate::supervisor::on_process_killed(vector);
        crate::thread::kill_current_and_reschedule();
        // Only reached in the degenerate case where NOTHING else was
        // runnable (shouldn't happen once thread 0 exists) -- still
        // fatal, just with an honest reason logged first.
        klog_error!("PROCESS_KILLED: nothing else was runnable, halting");
    }
    halt_forever();
}

/// Generates a diverging `x86-interrupt` handler: reports the fault, then
/// recovers (kills just the faulting process) or halts the whole kernel,
/// per `recover_or_halt`'s ring-3-vs-ring-0 rule.
macro_rules! handler_no_ec {
    ($name:ident, $vector:expr) => {
        extern "x86-interrupt" fn $name(frame: InterruptStackFrame) {
            report($vector, None, &frame);
            recover_or_halt($vector, &frame);
        }
    };
}

macro_rules! handler_with_ec {
    ($name:ident, $vector:expr) => {
        extern "x86-interrupt" fn $name(frame: InterruptStackFrame, error_code: u64) {
            report($vector, Some(error_code), &frame);
            recover_or_halt($vector, &frame);
        }
    };
}

handler_no_ec!(h_divide_error, 0);
handler_no_ec!(h_debug, 1);
handler_no_ec!(h_nmi, 2);
handler_no_ec!(h_breakpoint, 3);
handler_no_ec!(h_overflow, 4);
handler_no_ec!(h_bound_range, 5);
handler_no_ec!(h_invalid_opcode, 6);
handler_no_ec!(h_device_not_available, 7);

/// Deliberately NEVER recovers, even for a ring-3 CS — a double fault
/// means exception delivery ITSELF failed (see `docs/PROGRESS.md`'s
/// account of exactly this happening for a real reason earlier in this
/// project: a corrupted/unmapped stack at the moment of an original
/// fault). Whatever invariant let a normal fault be delivered safely
/// enough to recover from is exactly what's in question here — killing
/// "just the process" and continuing would be trusting a kernel-wide
/// invariant that has already been shown to be broken. Always fatal.
extern "x86-interrupt" fn h_double_fault(frame: InterruptStackFrame, error_code: u64) -> ! {
    report(8, Some(error_code), &frame);
    halt_forever();
}

handler_no_ec!(h_coprocessor_segment_overrun, 9);
handler_with_ec!(h_invalid_tss, 10);
handler_with_ec!(h_segment_not_present, 11);
handler_with_ec!(h_stack_fault, 12);
handler_with_ec!(h_general_protection, 13);

extern "x86-interrupt" fn h_page_fault(frame: InterruptStackFrame, error_code: u64) {
    // Earlier in this session this handler used crate::serial::write_hex_raw
    // directly (bypassing klog_error!/core::fmt) while root-causing a real
    // bug where the formatting path itself faulted — this handler kept
    // re-faulting on its own diagnostic call and never reached
    // halt_forever(). That root cause (a linker-symbol alignment bug, see
    // linker.ld) is fixed and covered by host_tests/; back to the normal
    // formatted report() every other handler uses, which is strictly more
    // informative (decodes present/write/user/reserved/instruction-fetch
    // bits, not just the raw values).
    let cr2 = read_cr2();
    klog_error!(
        "EXCEPTION vector=14 (PAGE FAULT) error_code=0x{:x} cr2=0x{:x} rip=0x{:x} rsp=0x{:x} present={} write={} user={} instr_fetch={}",
        error_code,
        cr2,
        frame.instruction_pointer,
        frame.stack_pointer,
        error_code & 1 != 0,
        error_code & 2 != 0,
        error_code & 4 != 0,
        error_code & 16 != 0,
    );
    recover_or_halt(14, &frame);
}

handler_no_ec!(h_reserved_15, 15);
handler_no_ec!(h_x87_fp, 16);
handler_with_ec!(h_alignment_check, 17);
handler_no_ec!(h_machine_check, 18);
handler_no_ec!(h_simd_fp, 19);
handler_no_ec!(h_virtualization, 20);
handler_with_ec!(h_control_protection, 21);
handler_no_ec!(h_reserved_22, 22);
handler_no_ec!(h_reserved_23, 23);
handler_no_ec!(h_reserved_24, 24);
handler_no_ec!(h_reserved_25, 25);
handler_no_ec!(h_reserved_26, 26);
handler_no_ec!(h_reserved_27, 27);
handler_no_ec!(h_hypervisor_injection, 28);
handler_with_ec!(h_vmm_communication, 29);
handler_with_ec!(h_security_exception, 30);
handler_no_ec!(h_reserved_31, 31);

/// Vector 0x20 (`apic::TIMER_VECTOR`) — a real interrupt handler that
/// does NOT halt (unlike every fault handler above). `apic::on_tick()`
/// increments a counter; nothing else yet, matching Phase 0's "deferred
/// interrupt work" principle from day one for the one interrupt source
/// that existed at the time (there's no rendering/parsing/scheduling
/// happening here to defer — this handler is already minimal, not a case
/// that needs fixing later like the C keyboard handler did).
extern "x86-interrupt" fn h_timer(_frame: InterruptStackFrame) {
    crate::apic::on_tick();
    crate::interrupt_forward::notify(crate::apic::TIMER_VECTOR);
    crate::thread::schedule();
}

/// Vector 0x21 (`pic::KEYBOARD_VECTOR`) — Phase 3's PS/2 keyboard driver.
/// Deliberately does NOT read the scancode itself (port 0x60) — that's
/// left entirely to the real ring-3 driver, which holds the actual
/// `PortIoRange` capability for it (see `user_rs/keyboard_driver`); this
/// handler's only job is the two things that MUST happen at kernel level
/// (real hardware EOI, so the PIC can ever raise IRQ1 again; and waking
/// whatever's blocked in `interrupt_forward::wait_for_interrupt`) — same
/// minimal-ISR philosophy `h_timer` already documents.
extern "x86-interrupt" fn h_keyboard(_frame: InterruptStackFrame) {
    crate::interrupt_forward::notify(crate::pic::KEYBOARD_VECTOR);
    crate::pic::send_eoi(1);
    crate::thread::schedule();
}

/// Vector 0x2C (`pic::MOUSE_VECTOR`) -- real PS/2 mouse (IRQ12, the
/// 8042's second/auxiliary port), same minimal-ISR discipline as
/// `h_keyboard` right above: real hardware EOI (IRQ12 lives on the
/// SLAVE PIC, so `send_eoi(12)` acknowledges BOTH PICs -- see that
/// function's own doc), wake whatever's blocked in `interrupt_forward::
/// wait_for_interrupt`, nothing else. The actual mouse-packet read
/// (port 0x60) is left entirely to the real ring-3 driver
/// (`user_rs/mouse_driver`), which holds the actual `PortIoRange`
/// capability for it.
extern "x86-interrupt" fn h_mouse(_frame: InterruptStackFrame) {
    crate::interrupt_forward::notify(crate::pic::MOUSE_VECTOR);
    crate::pic::send_eoi(12);
    crate::thread::schedule();
}

/// Phase 9 deliverable 3 (`docs/ROADMAP.md` §5 — "IPI-based reschedule"):
/// `smp::RESCHEDULE_VECTOR`, sent core-to-core via `apic::send_ipi_vector`
/// (`smp::send_reschedule_ipi`). A real, ordinary interrupt like any
/// other here — the ONLY thing distinguishing it from `h_timer` above is
/// what triggered it (another core, not this core's own LAPIC timer).
/// Minimal ISR, same discipline as every other handler in this file:
/// real hardware EOI, then hand off to `schedule()` immediately so the
/// thread that was just placed on THIS core's queue (`thread::
/// spawn_pinned_to_cpu`) gets a chance to run without waiting for this
/// core's own next periodic tick.
extern "x86-interrupt" fn h_reschedule(_frame: InterruptStackFrame) {
    crate::apic::eoi();
    crate::thread::schedule();
}

/// Phase 9 deliverable 4 (`docs/ROADMAP.md` §5 — "TLB shootdown via IPI
/// on every cross-core mapping change"): `smp::TLB_SHOOTDOWN_VECTOR`.
/// Executes a real `invlpg` for the vaddr the initiating core placed in
/// `smp::TLB_SHOOTDOWN_PENDING_VADDR` BEFORE sending this IPI (real,
/// necessary ordering — see `smp::shootdown_tlb`'s own doc comment for
/// why that write happens-before the IPI send, and why a single pending
/// slot is safe here specifically because shootdowns are serialized by
/// `smp::shootdown_tlb`'s own lock, never two in flight at once), then
/// atomically acknowledges via `smp::TLB_SHOOTDOWN_ACKS` so the
/// initiator's own bounded wait can observe real completion, not just
/// "the IPI was sent."
extern "x86-interrupt" fn h_tlb_shootdown(_frame: InterruptStackFrame) {
    crate::apic::eoi();
    crate::smp::handle_tlb_shootdown_ipi();
}

pub fn init() {
    set_entry(0, h_divide_error as *const () as u64, 0);
    set_entry(1, h_debug as *const () as u64, 0);
    set_entry(2, h_nmi as *const () as u64, 0);
    set_entry(3, h_breakpoint as *const () as u64, 0);
    set_entry(4, h_overflow as *const () as u64, 0);
    set_entry(5, h_bound_range as *const () as u64, 0);
    set_entry(6, h_invalid_opcode as *const () as u64, 0);
    set_entry(7, h_device_not_available as *const () as u64, 0);
    set_entry(8, h_double_fault as *const () as u64, DOUBLE_FAULT_IST_VALUE);
    set_entry(9, h_coprocessor_segment_overrun as *const () as u64, 0);
    set_entry(10, h_invalid_tss as *const () as u64, 0);
    set_entry(11, h_segment_not_present as *const () as u64, 0);
    set_entry(12, h_stack_fault as *const () as u64, 0);
    set_entry(13, h_general_protection as *const () as u64, 0);
    set_entry(14, h_page_fault as *const () as u64, 0);
    set_entry(15, h_reserved_15 as *const () as u64, 0);
    set_entry(16, h_x87_fp as *const () as u64, 0);
    set_entry(17, h_alignment_check as *const () as u64, 0);
    set_entry(18, h_machine_check as *const () as u64, 0);
    set_entry(19, h_simd_fp as *const () as u64, 0);
    set_entry(20, h_virtualization as *const () as u64, 0);
    set_entry(21, h_control_protection as *const () as u64, 0);
    set_entry(22, h_reserved_22 as *const () as u64, 0);
    set_entry(23, h_reserved_23 as *const () as u64, 0);
    set_entry(24, h_reserved_24 as *const () as u64, 0);
    set_entry(25, h_reserved_25 as *const () as u64, 0);
    set_entry(26, h_reserved_26 as *const () as u64, 0);
    set_entry(27, h_reserved_27 as *const () as u64, 0);
    set_entry(28, h_hypervisor_injection as *const () as u64, 0);
    set_entry(29, h_vmm_communication as *const () as u64, 0);
    set_entry(30, h_security_exception as *const () as u64, 0);
    set_entry(31, h_reserved_31 as *const () as u64, 0);
    set_entry(
        crate::apic::TIMER_VECTOR as usize,
        h_timer as *const () as u64,
        0,
    );
    set_entry(
        crate::pic::KEYBOARD_VECTOR as usize,
        h_keyboard as *const () as u64,
        0,
    );
    set_entry(
        crate::pic::MOUSE_VECTOR as usize,
        h_mouse as *const () as u64,
        0,
    );
    // Phase 9 deliverables 3 and 4 -- real core-to-core IPI vectors.
    set_entry(
        crate::smp::RESCHEDULE_VECTOR as usize,
        h_reschedule as *const () as u64,
        0,
    );
    set_entry(
        crate::smp::TLB_SHOOTDOWN_VECTOR as usize,
        h_tlb_shootdown as *const () as u64,
        0,
    );

    load_current_cpu();
    crate::klog_info!("IDT initialized: all 32 CPU exception vectors handled");
}

/// Phase 9 deliverable 2: loads THIS core's own IDTR to point at the
/// one, shared IDT table `init()` already built (the table's contents
/// are identical for every core -- the same handler addresses and the
/// same IST index -- only the CPU's own IDTR register is per-core
/// state, exactly like `gdt.rs`'s GDTR). The BSP reaches this via
/// `init()` above; every AP calls it directly (`smp.rs::ap_entry`)
/// once it's running, WITHOUT re-running `init()`'s entry-building
/// loop -- `bring_up_all` brings APs up one at a time (this module's
/// own sequencing discipline), but the shared `IDT` static is still
/// written by exactly one core (the BSP, once, before any AP exists),
/// so no AP ever needs or should rebuild it.
pub fn load_current_cpu() {
    unsafe {
        let idtr = IdtDescriptor {
            limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
            base: (&raw const IDT) as u64,
        };
        core::arch::asm!("lidt [{}]", in(reg) &idtr, options(readonly, nostack, preserves_flags));
    }
}
