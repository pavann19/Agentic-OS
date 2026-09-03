//! Ring 3 execution. Phase 1 item — nothing in this kernel has ever run
//! below CPL0 before this. `docs/ROADMAP.md`'s User/Kernel Boundary target
//! starts here: "User programs run in ring 3."
//!
//! Proof strategy: rather than needing syscalls (a separate, not-yet-built
//! Phase 1 item) just to get user code to prove itself, the demo user
//! program executes a single `hlt` — a privileged instruction, CPL0-only.
//! If ring 3 is real, the CPU takes a #GP fault the instant it tries. A
//! `#GP` reported by `idt.rs` IS the proof: kernel code executing the
//! identical instruction never faults, so a fault here means CPL was
//! genuinely 3, not just "we jumped to a different address while staying
//! at CPL0."

use crate::gdt::{USER_CODE_SELECTOR, USER_DATA_SELECTOR};

/// Builds the iretq frame and drops to ring 3 at `entry` with `user_rsp`
/// as the user stack pointer. Never returns (whatever runs at `entry`
/// either loops forever or, for the demo, faults back into the kernel via
/// an exception — either way, control doesn't come back here).
pub unsafe fn enter_user_mode(entry: u64, user_rsp: u64) -> ! {
    core::arch::asm!(
        "push {ss}",       // SS (user data selector | RPL 3)
        "push {rsp}",      // user RSP
        "push {rflags}",   // RFLAGS: IF=1 (0x202) -- interrupts must stay
                            // enabled in user mode, or a runaway user
                            // program could never be preempted at all
        "push {cs}",       // CS (user code selector | RPL 3)
        "push {rip}",      // RIP -- where execution starts
        "iretq",
        ss = in(reg) USER_DATA_SELECTOR as u64,
        rsp = in(reg) user_rsp,
        rflags = in(reg) 0x202u64,
        cs = in(reg) USER_CODE_SELECTOR as u64,
        rip = in(reg) entry,
        options(noreturn)
    );
}
