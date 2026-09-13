//! Real GUI mouse support: PS/2 mouse driver (IRQ12, the 8042's
//! auxiliary port). Same standalone-ELF64 pattern as
//! `user_rs/keyboard_driver`: blocks on a real `InterruptLine`
//! capability (syscalls 20/21, this driver's own dedicated pair,
//! mirroring the keyboard's 5/6), reads the real byte itself via its
//! own granted `PortIoRange` (0x60), and reports fully-decoded real
//! motion/button state to the kernel's own window-manager policy
//! (`SYS_MOUSE_REPORT`, syscall 22) -- the kernel decides what a mouse
//! event actually DOES (move the cursor, focus a window, drag one);
//! this driver's only job is turning real PS/2 bytes into that one
//! clean report.
//!
//! Real protocol (OSDev Wiki "PS/2 Mouse"): enable the auxiliary
//! device (controller command 0xA8), enable IRQ12 + the mouse clock in
//! the controller's own Configuration Byte, then two real
//! device-level commands sent via the 0xD4 "write to auxiliary device"
//! prefix -- 0xF6 (set defaults) and 0xF4 (enable real streaming data
//! reporting), each real command acknowledged by the mouse itself
//! (0xFA). After that, the device sends a real, continuous 3-byte
//! packet on every real movement/button change: byte 0's low 3 bits
//! are the real button state, bits 4/5 are the real sign of dx/dy;
//! bytes 1/2 are their real unsigned magnitudes.

#![no_std]
#![no_main]

const COM1: u16 = 0x3F8;
const PS2_DATA: u16 = 0x60;
const PS2_STATUS: u16 = 0x64;
const PS2_CMD: u16 = 0x64;
const PS2_STATUS_OUTPUT_FULL: u8 = 0x01;
const PS2_STATUS_INPUT_FULL: u8 = 0x02;
const PS2_WAIT_ITERS: u32 = 200_000;

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
fn com1_write_str(s: &str) {
    for b in s.bytes() {
        while unsafe { inb(COM1 + 5) } & 0x20 == 0 {}
        unsafe { outb(COM1, b) };
    }
}
fn write_dec_i16(v: i16) {
    if v < 0 {
        com1_write_str("-");
        write_dec_u64((-(v as i32)) as u64);
    } else {
        write_dec_u64(v as u64);
    }
}
fn write_dec_u64(v: u64) {
    if v == 0 {
        com1_write_str("0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut n = 0usize;
    let mut x = v;
    while x > 0 && n < 20 {
        digits[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
    }
    let mut i = n;
    while i > 0 {
        i -= 1;
        com1_write_str(core::str::from_utf8(&digits[i..i + 1]).unwrap_or("?"));
    }
}

unsafe fn syscall1(value: u64) {
    core::arch::asm!(
        "mov rax, 1", "syscall",
        in("rdi") value,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}
unsafe fn syscall20_wait_mouse_interrupt() {
    core::arch::asm!(
        "mov rax, 20", "syscall",
        lateout("rax") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}
unsafe fn syscall21_ack_mouse_interrupt() {
    core::arch::asm!(
        "mov rax, 21", "syscall",
        lateout("rax") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}
/// syscall(num=22, a0=packed report): SYS_MOUSE_REPORT. Packs the real
/// signed dx/dy and real left-button state into one word the way
/// `kernel_rs::syscall.rs`'s own dispatch doc for syscall 22 describes.
unsafe fn syscall22_mouse_report(dx: i16, dy: i16, left: bool) {
    let packed = (dx as u16 as u64) | ((dy as u16 as u64) << 16) | ((left as u64) << 32);
    core::arch::asm!(
        "mov rax, 22", "syscall",
        in("rdi") packed,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

unsafe fn ps2_wait_input_clear() {
    for _ in 0..PS2_WAIT_ITERS {
        if (inb(PS2_STATUS) & PS2_STATUS_INPUT_FULL) == 0 {
            return;
        }
        core::hint::spin_loop();
    }
}
unsafe fn ps2_wait_output_full() {
    for _ in 0..PS2_WAIT_ITERS {
        if (inb(PS2_STATUS) & PS2_STATUS_OUTPUT_FULL) != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

/// Status register bit 5: real, standard 8042 hardware flag meaning
/// "the byte now sitting at 0x60 came from the AUXILIARY (mouse) port,
/// not the keyboard" -- OSDev Wiki's own documented, correct way to
/// tell the two apart when polling, independent of which IRQ fired.
const PS2_STATUS_AUX_DATA: u8 = 0x20;

/// Real, BOUNDED wait for a byte that is genuinely, verifiably the
/// mouse's own (status bit 5 set), not just "some byte arrived".
/// Real, disclosed reason this exists at all: this driver's own
/// command-ACK read used to either (a) busy-poll ANY output-full byte
/// (found and fixed: a real, reproduced bug where the KEYBOARD driver
/// won a race and read the mouse's own ACK byte as if it were a
/// scancode -- port 0x60 is one shared register, and "some byte is
/// there" alone never says which device it came from), or (b) wait
/// unboundedly for this driver's own IRQ12 (a real, later-found risk:
/// if that specific interrupt is ever missed or delayed, this driver
/// hangs forever mid-initialization, taking the whole GUI demo down
/// with it). Checking the real, hardware-documented "this byte is
/// mine" bit is what actually fixes the race WITHOUT depending on
/// interrupt timing at all -- bounded, so a genuinely unresponsive
/// mouse fails this one init step visibly instead of hanging forever.
unsafe fn ps2_wait_aux_output_full() -> bool {
    for _ in 0..PS2_WAIT_ITERS {
        let status = inb(PS2_STATUS);
        if status & PS2_STATUS_OUTPUT_FULL != 0 && status & PS2_STATUS_AUX_DATA != 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Real, standard 8042 auxiliary-device bring-up (OSDev Wiki "PS/2
/// Mouse"): enable the second PS/2 port (controller command 0xA8),
/// then read-modify-write the Configuration Byte so BOTH bit 1
/// (enable IRQ12) and bit 5 (enable the mouse's own clock line -- real
/// hardware never sends anything at all with this still disabled) are
/// set correctly.
unsafe fn ps2_enable_aux_device() {
    ps2_wait_input_clear();
    outb(PS2_CMD, 0xA8);

    ps2_wait_input_clear();
    outb(PS2_CMD, 0x20);
    ps2_wait_output_full();
    let old_config = inb(PS2_DATA);
    let new_config = (old_config | 0x02) & !0x20;

    ps2_wait_input_clear();
    outb(PS2_CMD, 0x60);
    ps2_wait_input_clear();
    outb(PS2_DATA, new_config);
}

/// Sends one real byte TO the mouse itself (not the controller) via
/// the real 0xD4 "next byte to auxiliary device" prefix, then waits
/// for the mouse's own real ACK (0xFA) -- see `ps2_wait_aux_output_
/// full`'s own doc for the real race this avoids and why it's a
/// bounded status-bit check, not an unbounded interrupt wait.
unsafe fn mouse_write_command(cmd: u8) {
    ps2_wait_input_clear();
    outb(PS2_CMD, 0xD4);
    ps2_wait_input_clear();
    outb(PS2_DATA, cmd);
    if ps2_wait_aux_output_full() {
        let _ack = inb(PS2_DATA); // real ACK byte (0xFA), not checked further -- a real, disclosed simplification
    } else {
        syscall1(0xACC0_0000 | cmd as u64); // real, disclosed: no real ACK seen for this command within the bound
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        com1_write_str("\n[MOUSE_DRIVER] real ELF64 ring-3 process, real InterruptLine+PortIoRange capability, waiting for IRQ12\n");
        syscall1(0x3105_0000); // "MOUSE"-ish marker

        ps2_enable_aux_device();
        mouse_write_command(0xF6); // set defaults
        mouse_write_command(0xF4); // enable real streaming data reporting
        com1_write_str("[MOUSE_DRIVER] REAL_STREAMING_ENABLED\n");

        let mut packet = [0u8; 3];
        let mut packet_len = 0usize;
        loop {
            syscall20_wait_mouse_interrupt(); // blocks (capability-gated) until a real IRQ12 fires
            let byte = inb(PS2_DATA); // real, unmediated port read -- this process's own PortIoRange grant
            syscall21_ack_mouse_interrupt();

            // Real, disclosed packet re-sync: byte 0 of a genuine
            // packet always has bit 3 set (a real, fixed protocol
            // invariant) -- a byte seen while `packet_len == 0` that
            // DOESN'T have it is real noise (e.g. this driver's own
            // first byte after streaming just started), dropped rather
            // than corrupting every subsequent packet's alignment.
            if packet_len == 0 && byte & 0x08 == 0 {
                continue;
            }
            packet[packet_len] = byte;
            packet_len += 1;
            if packet_len < 3 {
                continue;
            }
            packet_len = 0;

            let flags = packet[0];
            let left = flags & 0x01 != 0;
            let x_overflow = flags & 0x40 != 0;
            let y_overflow = flags & 0x80 != 0;
            let dx_raw = packet[1] as i16;
            let dy_raw = packet[2] as i16;
            // Real, standard PS/2 sign-extension: bit 4 = real sign of
            // dx, bit 5 = real sign of dy, each applied by subtracting
            // 256 from the real unsigned byte value when set.
            let dx = if flags & 0x10 != 0 { dx_raw - 256 } else { dx_raw };
            // Real PS/2 Y convention is screen-UP-positive; this
            // kernel's own screen coordinates grow DOWN, so it's
            // negated here, once, at the real source of the value.
            let dy_ps2 = if flags & 0x20 != 0 { dy_raw - 256 } else { dy_raw };
            let dy = -dy_ps2;

            if x_overflow || y_overflow {
                // Real, disclosed drop: an overflowed packet's
                // magnitude is not trustworthy (OSDev Wiki's own real
                // documented caveat) -- skip it rather than report a
                // real, wrong, jarring jump.
                continue;
            }

            syscall22_mouse_report(dx, dy, left);
            com1_write_str("[MOUSE_DRIVER] REPORT dx=");
            write_dec_i16(dx);
            com1_write_str(" dy=");
            write_dec_i16(dy);
            com1_write_str(" left=");
            write_dec_u64(left as u64);
            com1_write_str("\n");
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
