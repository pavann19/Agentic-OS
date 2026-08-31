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
//! Honesty note (real, disclosed limitation, not glossed over): the
//! automated headless QEMU test harness this project's `test-boot.ps1`
//! uses has no way to synthesize a real PS/2 keystroke (no monitor/HMP
//! scripting wired in, `-display none`) — so this process will correctly
//! sit blocked in syscall 5 forever in every automated CI-style run, and
//! that's the RIGHT behavior for a real, correctly-waiting driver with no
//! input to react to, not a bug. The mechanism itself (unmasked IRQ1,
//! the real IDT vector, the real capability grant, the real syscalls) is
//! exercised and verified up to exactly the point where it starts
//! waiting; the full keystroke-to-scancode path is real code, verified
//! by inspection and by the same techniques that verified every other
//! syscall path in this kernel, but not exercised by an actual interrupt
//! in this project's current automated regression suite.

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

const PS2_DATA_PORT: u16 = 0x60;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1_write_str("\n[KEYBOARD_DRIVER] real ELF64 ring-3 process, real InterruptLine+PortIoRange capability, waiting for IRQ1\n");
    unsafe { syscall1(0xB0AD) }; // "keyBOARD"-ish marker, distinct from the other two drivers' (0xD067, 0xF6)
    loop {
        unsafe {
            syscall5_wait_kbd_interrupt(); // blocks (capability-gated) until a real IRQ1 fires
            let scancode = inb(PS2_DATA_PORT); // real, unmediated port read -- this process's own PortIoRange grant
            syscall1(0xB0_0000 | scancode as u64); // logs the real scancode via the kernel's own klog path
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
