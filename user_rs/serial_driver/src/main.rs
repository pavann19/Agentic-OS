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

#![no_std]
#![no_main]

const COM1: u16 = 0x3F8;

#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

fn tx_ready() -> bool {
    unsafe { (inb(COM1 + 5) & 0x20) != 0 }
}

fn write_byte(b: u8) {
    while !tx_ready() {}
    unsafe { outb(COM1, b) };
}

fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

/// syscall(num=1, a0=value): the kernel's existing "log a value" syscall
/// -- reused here (not a new syscall) specifically so this real ELF
/// process's proof-of-life is visible via the normal klog path too, as a
/// second, independent confirmation alongside the raw COM1 bytes above.
unsafe fn syscall1(value: u64) {
    // Real bug found and fixed on virtio_blk_driver (first driver whose
    // code relied on a register surviving a syscall -- see that crate's
    // module doc): SYSCALL/SYSRET does NOT save/restore general-purpose
    // registers the way an interrupt/iretq does, and the kernel's own
    // syscall_dispatch is a normal extern "C" fn free to clobber every
    // System V caller-saved register (rdi/rsi/rdx/rcx/r8-r11), not just
    // the two (rcx/r11) the hardware itself repurposes. Marking only
    // those two as clobbered (as this function used to) let the compiler
    // believe rsi/rdx/r8/r9/r10 survive a syscall unchanged -- true only
    // by accident whenever nothing after the call happens to still need
    // them, which was true for every prior driver until virtio_blk_driver
    // wasn't. Full clobber list now, matching the real ABI.
    core::arch::asm!(
        "syscall",
        inout("rax") 1u64 => _,
        in("rdi") value,
        lateout("rsi") _,
        lateout("rdx") _,
        lateout("rcx") _,
        lateout("r8") _,
        lateout("r9") _,
        lateout("r10") _,
        lateout("r11") _,
        options(nostack)
    );
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("\n[USERSPACE_SERIAL_DRIVER] real ELF64 ring-3 process, real PortIoRange capability, real COM1 write\n");
    unsafe { syscall1(0xD067) }; // "D067" ~ "driver" -- distinguishes this real-ELF process's syscall from the hand-built demos' markers (0xCAFE, 0x1234, 0xC0DE)
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
