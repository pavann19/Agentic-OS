//! Same COM1 bring-up as `kernel_rs/src/serial.rs` / the old `boot/main.c`'s
//! `boot_serial_*` helpers. Duplicated rather than shared as a crate dependency
//! for now — the bootloader and kernel are separate binaries with separate
//! target specs (PE/EFI vs. bare-metal ELF), and this is 20 lines. Revisit as
//! a shared crate if it grows past "trivial" before Phase 1.

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

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1 + 0, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

fn is_ready() -> bool {
    unsafe { (inb(COM1 + 5) & 0x20) != 0 }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            while !is_ready() {}
            unsafe { outb(COM1, b'\r') };
        }
        while !is_ready() {}
        unsafe { outb(COM1, b) };
    }
}
