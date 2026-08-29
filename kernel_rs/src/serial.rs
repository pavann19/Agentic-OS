//! Port of `kernel/serial.c`. Same COM1 UART bring-up sequence, same port
//! numbers, same 16550-compatible init bytes — a 1:1 behavioral port, not
//! a redesign, so the existing `_evidence/latest/serial.log` checkpoint
//! format keeps working unchanged.

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

pub fn write_char(c: u8) {
    while !is_ready() {}
    unsafe { outb(COM1, c) };
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_char(b'\r');
        }
        write_char(b);
    }
}

pub struct SerialWriter;

impl core::fmt::Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        write_str(s);
        Ok(())
    }
}
