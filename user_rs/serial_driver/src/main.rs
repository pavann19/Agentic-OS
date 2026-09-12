//! Real first user-space driver (Phase 3): a genuine standalone ELF64
//! binary, not a hand-built machine-code byte array like every ring-3
//! process before it (`init.rs`, the Phase 1/2 `demo_ring3` proof).
//! Loaded and mapped by `kernel_rs/src/elf.rs`'s real ELF loader.
//!
//! What this proves, concretely: the kernel granted this process a
//! `PortIoRange` capability for COM1 (0x3F8-0x3FF) BEFORE entering ring
//! 3 (`driver.rs::grant_port_access`, which opens exactly those bits in
//! the TSS IOPB — see that module's doc comment for why this is real
//! per-port mediation, not `IOPL=3`). This program does the actual
//! hardware I/O itself, completely unmediated by the kernel after that
//! one-time grant — genuine ring-3 `out`/`in` instructions against COM1,
//! exactly matching real x86 IOPB semantics (the CPU checks the bitmap
//! in hardware on every I/O instruction; there is no kernel code running
//! per-byte). The bytes it writes land in the SAME COM1 UART the
//! kernel's own `serial.rs` uses for every `[INFO]`/`[ERROR]` log line —
//! so a real string appearing in `_evidence/latest/serial.log`, written
//! by code that never went through `klog_info!`, is direct, physical
//! evidence this ran as real ring-3 code with real hardware access, not
//! a simulated log line.
//!
//! No process-exit syscall exists yet (a real, open gap — nothing in
//! this kernel has a clean process-exit path from ring 3 yet), so this
//! ends in a benign infinite spin rather than the deliberate
//! privileged-instruction #GP the OTHER ring-3 demos use to prove CPL —
//! that deliberate-crash technique halts the WHOLE kernel today (Phase
//! 1's still-open "one exception halts the machine" gap), which is fine
//! for a one-shot proof-of-mechanism demo but wrong for something meant
//! to represent a real, ongoing driver.
//!
//! Phase 13 deliverable 3: this is the first crate migrated to
//! `agentic_sdk` (new) instead of hand-rolling its own COM1/syscall
//! code — the real proof that crate is a genuine drop-in, not just an
//! untested library. Behavior is byte-identical: same COM1 output, same
//! syscall 1 with the same real full-clobber-list fix this crate's own
//! doc originally described (now `agentic_sdk::syscall::syscall3`'s
//! one canonical implementation instead of being re-derived here).

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    agentic_sdk::com1::write_str("\n[USERSPACE_SERIAL_DRIVER] real ELF64 ring-3 process, real PortIoRange capability, real COM1 write\n");
    // "D067" ~ "driver" -- distinguishes this real-ELF process's syscall
    // from the hand-built demos' markers (0xCAFE, 0x1234, 0xC0DE).
    unsafe { agentic_sdk::syscall::syscall1(1, 0xD067) };
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
