//! Real, from-hardware COM1 (0x3F8) serial I/O — byte-identical to every
//! `user_rs/*_driver` crate's own hand-copied version (e.g.
//! `window_client_driver`'s, before this crate existed). Requires the
//! calling process to already hold a real, granted `PortIoRange`
//! capability covering 0x3F8-0x3FF (`installer.rs`/`user_driver.rs`
//! grant it via `driver::grant_port_access` before ring 3 is ever
//! entered) — this module performs no capability check of its own; an
//! ungranted `out`/`in` here raises a real #GP, exactly the same
//! hardware-enforced boundary the TSS IOPB always has, capability or
//! not.

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

/// Real terminal-friendly newline translation (`\n` -> `\r\n`), matching
/// `serial_driver`'s own original convention — kept here rather than
/// dropped, since every serial log this project's own test scripts grep
/// already assumes it.
pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

/// Real, allocation-free decimal formatting — the same small routine
/// `netstack_driver`'s own `write_dec_u32` and `window_client_driver`'s
/// own `write_dec_u64` each hand-rolled separately; one real
/// implementation here, generalized to `u64` (a strict superset of the
/// `u32` case).
pub fn write_dec_u64(v: u64) {
    if v == 0 {
        write_str("0");
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
        write_str(core::str::from_utf8(&digits[i..i + 1]).unwrap_or("?"));
    }
}
