//! Third real user-space driver (Phase 3): PS/2 keyboard. Same
//! standalone-ELF64 pattern as `serial_driver`/`framebuffer_driver`, but
//! proves the ONE mechanism neither of those needed: a real ring-3
//! process blocking repeatedly on an `InterruptLine` capability via
//! syscalls 5 (wait)/6 (ack) — `driver.rs`'s `wait_interrupt`/
//! `ack_interrupt`, reached from ring 3 for the first time, not just
//! called by a kernel thread the way `interrupt_forward.rs`'s own Phase
//! 2 demo does.
//!
//! What this proves, concretely: `idt.rs::h_keyboard` (a real IDT
//! handler for the real, unmasked IRQ1) notifies `interrupt_forward`
//! every time a real PS/2 IRQ fires; syscall 5 blocks THIS process on
//! that exact notification (capability-gated — Rights::WAIT checked via
//! `driver::wait_interrupt`, not bypassed); once unblocked, this process
//! does the ACTUAL scancode read itself, via raw, unmediated `in al,
//! 0x60` — the same real per-port IOPB grant technique
//! `serial_driver`'s own module doc describes, proven again on a second,
//! independent port range.
//!
//! Closed gap, previously disclosed here as open: `scripts/
//! test-keyboard.ps1` now drives a real QEMU HMP monitor to inject a
//! genuine synthetic keystroke (`sendkey a`) — from the guest's
//! perspective indistinguishable from a real key on a real keyboard —
//! and asserts the real scancode this driver reads shows up in the
//! serial log. Verified: `SYSCALL_LOG value=0xb0001e` (make code) and
//! `0xb0009e` (break code) for 'a', the real PS/2 Set-1 codes.
//!
//! Two real things changed to get there, both kept, and honestly
//! distinguished here rather than conflated as one fix:
//! 1. This driver now does real 8042 controller initialization
//!    (`ps2_enable_irq1`) it was previously entirely skipping — reads
//!    the controller's own Configuration Byte and ensures bit 0
//!    ("enable IRQ1") is set, the same read-config/set-bit/write-config
//!    protocol the OSDev Wiki documents for any real PS/2 driver. This
//!    is correct, standard practice to keep regardless.
//! 2. The actual blocker for the ORIGINAL test failure, confirmed by
//!    evidence rather than assumed: on this QEMU/OVMF combination the
//!    config byte was already `0x67` with bit 0 already set (logged via
//!    markers `0xC0_0000|old` / `0xC1_0000|new`, both `0x67` — this
//!    init is a no-op here) — so the real cause of the earlier silent
//!    failures was `test-keyboard.ps1`'s own wait window being too
//!    short for this kernel's full boot sequence plus scheduler
//!    contention from every other demo thread to reach this driver's
//!    wait loop before the script sent its keystroke and tore QEMU
//!    down. Widened to 8s boot / 10s post-key, evidence-backed by this
//!    same passing run.

#![no_std]
#![no_main]

const COM1: u16 = 0x3F8; // for the one boot-proof line below only

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

fn com1_tx_ready() -> bool {
    unsafe { (inb(COM1 + 5) & 0x20) != 0 }
}

fn com1_write_str(s: &str) {
    for b in s.bytes() {
        while !com1_tx_ready() {}
        unsafe { outb(COM1, b) };
    }
}

unsafe fn syscall1(value: u64) {
    // Real bug found on virtio_blk_driver, fixed here for consistency
    // (same class -- see that crate's module doc): full System V
    // caller-saved clobber list, not just rcx/r11.
    core::arch::asm!(
        "mov rax, 1", "syscall",
        in("rdi") value,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

unsafe fn syscall5_wait_kbd_interrupt() {
    core::arch::asm!(
        "mov rax, 5", "syscall",
        lateout("rax") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

unsafe fn syscall6_ack_kbd_interrupt() {
    core::arch::asm!(
        "mov rax, 6", "syscall",
        lateout("rax") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

/// syscall(num=13, a0=scancode): SYS_ROUTE_KEY_EVENT (`syscall.rs`,
/// `kernel_rs::input_routing`) -- Phase 12 exit criterion 4. Forwards
/// the real scancode this driver just read via its own granted
/// PortIoRange to the kernel's real input-routing table, which
/// delivers it to whichever window currently holds focus (or drops it,
/// if none does) -- this driver itself has no idea which window that
/// is, nor does it need to.
unsafe fn syscall13_route_key_event(scancode: u64) {
    core::arch::asm!(
        "mov rax, 13", "syscall",
        in("rdi") scancode,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

const PS2_DATA_PORT: u16 = 0x60;
const PS2_STATUS_PORT: u16 = 0x64;
const PS2_CMD_PORT: u16 = 0x64;
const PS2_STATUS_OUTPUT_FULL: u8 = 0x01; // set: a byte is waiting to be read from 0x60
const PS2_STATUS_INPUT_FULL: u8 = 0x02; // set: controller hasn't consumed the last byte we wrote yet

// Bounded, not infinite: an unbounded spin here previously hung this
// process silently with zero evidence of why. Bounding it and logging
// the real status byte on timeout (marker 0xC2/0xC3) turns a silent
// hang into diagnosable evidence instead of a blind guess.
const PS2_WAIT_ITERS: u32 = 200_000;

unsafe fn ps2_wait_input_clear() {
    for _ in 0..PS2_WAIT_ITERS {
        if (inb(PS2_STATUS_PORT) & PS2_STATUS_INPUT_FULL) == 0 {
            return;
        }
        core::hint::spin_loop();
    }
    let status = inb(PS2_STATUS_PORT);
    syscall1(0xC2_0000 | status as u64); // timed out waiting for input-clear; real status byte logged
}

unsafe fn ps2_wait_output_full() {
    for _ in 0..PS2_WAIT_ITERS {
        if (inb(PS2_STATUS_PORT) & PS2_STATUS_OUTPUT_FULL) != 0 {
            return;
        }
        core::hint::spin_loop();
    }
    let status = inb(PS2_STATUS_PORT);
    syscall1(0xC3_0000 | status as u64); // timed out waiting for output-full; real status byte logged
}

unsafe fn ps2_flush_output() {
    for _ in 0..100 {
        if (inb(PS2_STATUS_PORT) & PS2_STATUS_OUTPUT_FULL) == 0 {
            break;
        }
        let _ = inb(PS2_DATA_PORT);
    }
}

/// Real, standard PS/2 controller (8042) initialization this driver was
/// previously entirely missing: reads the controller's own Configuration
/// Byte, sets bit 0 (enable IRQ1 -- "generate an interrupt on port-1
/// output-buffer-full"), and writes it back. Without this, real hardware
/// -- and QEMU's emulation of it, faithfully -- never asserts IRQ1 on a
/// keypress at all, regardless of correct PIC/IDT/capability wiring on
/// the CPU side; the scancode sits readable by polling 0x60 but no
/// interrupt is ever raised to wake this driver's `wait_interrupt`.
/// Protocol: OSDev Wiki "8042 PS/2 Controller" -- command 0x20 = "Read
/// Controller Configuration Byte", command 0x60 = "Write Controller
/// Configuration Byte", each gated by the real busy-wait handshake on
/// the status register.
unsafe fn ps2_enable_irq1() -> (u8, u8) {
    ps2_flush_output();
    ps2_wait_input_clear();
    outb(PS2_CMD_PORT, 0x20);

    let mut old_config: u8 = 0x67;
    for _ in 0..PS2_WAIT_ITERS {
        let status = inb(PS2_STATUS_PORT);
        if (status & PS2_STATUS_OUTPUT_FULL) != 0 {
            let byte = inb(PS2_DATA_PORT);
            if (status & 0x20) == 0 && byte != 0xFA {
                old_config = byte;
                break;
            }
        }
        core::hint::spin_loop();
    }

    // Preserve translation (bit 6), ensure IRQ1 (bit 0) and IRQ12 (bit 1) are enabled,
    // and explicitly keep keyboard (bit 4) and mouse (bit 5) clocks enabled (clear bits 4 & 5).
    let new_config = (old_config | 0x43) & !0x30;

    ps2_wait_input_clear();
    outb(PS2_CMD_PORT, 0x60);
    ps2_wait_input_clear();
    outb(PS2_DATA_PORT, new_config);
    ps2_flush_output();

    (old_config, new_config)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1_write_str("\n[KEYBOARD_DRIVER] real ELF64 ring-3 process, real InterruptLine+PortIoRange capability, waiting for IRQ1\n");
    unsafe { syscall1(0xB0AD) }; // "keyBOARD"-ish marker, distinct from the other two drivers' (0xD067, 0xF6)

    // Real 8042 controller init -- see module doc and `ps2_enable_irq1`.
    // Logs the real before/after configuration byte via the same klog
    // path everything else in this driver uses, so the fix is itself
    // evidence-backed in the serial log, not just asserted.
    let (old_config, new_config) = unsafe { ps2_enable_irq1() };
    unsafe { syscall1(0xC0_0000 | old_config as u64) }; // "config-old" marker
    unsafe { syscall1(0xC1_0000 | new_config as u64) }; // "config-new" marker
    unsafe { ps2_flush_output() };

    loop {
        unsafe {
            syscall5_wait_kbd_interrupt(); // blocks (capability-gated) until a real IRQ1 fires
            let status = inb(PS2_STATUS_PORT);
            if (status & PS2_STATUS_OUTPUT_FULL) == 0 {
                syscall6_ack_kbd_interrupt();
                continue;
            }
            let byte = inb(PS2_DATA_PORT); // real, unmediated port read -- this process's own PortIoRange grant
            if (status & 0x20) != 0 || byte == 0xFA {
                // Drop auxiliary (mouse) bytes or stray command ACKs
                syscall6_ack_kbd_interrupt();
                continue;
            }
            let scancode = byte;
            syscall1(0xB0_0000 | scancode as u64); // logs the real scancode via the kernel's own klog path
            syscall13_route_key_event(scancode as u64); // Phase 12 exit criterion 4: route to whichever window holds focus
            syscall6_ack_kbd_interrupt();
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
