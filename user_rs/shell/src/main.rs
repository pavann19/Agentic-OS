//! Phase 7's text shell (`docs/ROADMAP.md` §5 Phase 7, deliverable 1):
//! "a user-space process — inspect processes, memory, capabilities,
//! devices, storage, and audit log." A real freestanding ELF64 ring-3
//! process, no different in kind from any driver crate in this repo.
//!
//! **Capability-aware, not privileged** (deliverable 2): every command
//! this shell offers is a thin, honest wrapper around a real,
//! already-capability-gated Phase 5 syscall — `ps` is syscall 7
//! (structured introspection), `tools` is syscall 8 (the tool/intent
//! catalog), `audit` is syscall 9 (this process's OWN audit trail).
//! This process holds exactly the capabilities `kernel_rs/src/shell.rs`
//! granted it before it ever started running — the same
//! `thread::spawn_with_capabilities` mechanism Phase 5's agent demo
//! uses — and nothing about being "the shell" gives it a privileged
//! path around those checks. `rawin` demonstrates the boundary
//! concretely: an attempt to read a port this process was never granted
//! (anything outside COM1's 8 ports) hits the real TSS IOPB and faults
//! — this kernel's own ring-3 fault isolation
//! (`idt.rs::recover_or_halt`) then kills THIS PROCESS ALONE, real,
//! hardware-enforced evidence for "shell operations exceeding its
//! capabilities are denied," not a graceful in-band error this code
//! chose to print.
//!
//! Real bidirectional COM1 (every other driver in this repo only ever
//! WRITES to its own COM1 debug line) — `read_line` polls the UART's
//! Line Status Register for real data-ready bits and echoes every typed
//! character back, the same real UART protocol any serial terminal
//! program speaks, not a simulated line editor.

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

fn com1_tx_ready() -> bool {
    unsafe { (inb(COM1 + 5) & 0x20) != 0 }
}
fn com1_rx_ready() -> bool {
    unsafe { (inb(COM1 + 5) & 0x01) != 0 }
}

fn write_byte(b: u8) {
    while !com1_tx_ready() {}
    unsafe { outb(COM1, b) };
}
fn write_str(s: &str) {
    for b in s.bytes() {
        write_byte(b);
    }
}
fn read_byte() -> u8 {
    while !com1_rx_ready() {}
    unsafe { inb(COM1) }
}

fn write_u64_dec(mut v: u64) {
    if v == 0 {
        write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 20;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        write_byte(b);
    }
}

fn write_hex_u64(v: u64) {
    write_str("0x");
    let mut started = false;
    for shift in (0..16).rev() {
        let nibble = ((v >> (shift * 4)) & 0xF) as u8;
        if nibble != 0 || started || shift == 0 {
            started = true;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + (nibble - 10) };
            write_byte(c);
        }
    }
}

unsafe fn syscall_buf(num: u64, buf: u64, max_entries: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "mov rax, {num}", "syscall",
        num = in(reg) num,
        in("rdi") buf, in("rsi") max_entries,
        lateout("rax") ret, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
    ret
}

const MAX_ENTRIES: usize = 16;
const THREAD_INFO_SIZE: usize = 16;
const TOOL_DESCRIPTOR_SIZE: usize = 16;
const AUDIT_ENTRY_SIZE: usize = 24;
const BUF_BYTES: usize = MAX_ENTRIES * AUDIT_ENTRY_SIZE;

#[repr(C, align(8))]
struct SharedBuf {
    bytes: [u8; BUF_BYTES],
}
static mut BUF: SharedBuf = SharedBuf { bytes: [0u8; BUF_BYTES] };

unsafe fn read_u64_le(base: *const u8, offset: usize) -> u64 {
    let mut v: u64 = 0;
    for i in 0..8 {
        v |= (core::ptr::read_volatile(base.add(offset + i)) as u64) << (i * 8);
    }
    v
}
unsafe fn read_u32_le(base: *const u8, offset: usize) -> u32 {
    let mut v: u32 = 0;
    for i in 0..4 {
        v |= (core::ptr::read_volatile(base.add(offset + i)) as u32) << (i * 8);
    }
    v
}
unsafe fn buf_base() -> *const u8 {
    core::ptr::addr_of!(BUF.bytes) as *const u8
}

const THREAD_STATE_NAMES: [&str; 3] = ["Ready", "Running", "Exited"];

fn cmd_ps() {
    let buf_addr = unsafe { core::ptr::addr_of!(BUF.bytes) as u64 };
    let result = unsafe { syscall_buf(7, buf_addr, MAX_ENTRIES as u64) };
    if result == u64::MAX {
        write_str("permission denied: this shell was not granted Rights::INTROSPECT\r\n");
        return;
    }
    write_str("  PID  STATE    USER\r\n");
    let count = core::cmp::min(result as usize, MAX_ENTRIES);
    for i in 0..count {
        let base = unsafe { buf_base().add(i * THREAD_INFO_SIZE) };
        let id = unsafe { read_u64_le(base, 0) };
        let state = unsafe { read_u32_le(base, 8) } as usize;
        let is_user = unsafe { read_u32_le(base, 12) };
        write_str("  ");
        write_u64_dec(id);
        write_str("    ");
        write_str(if state < 3 { THREAD_STATE_NAMES[state] } else { "?" });
        write_str("    ");
        write_str(if is_user != 0 { "yes" } else { "no" });
        write_str("\r\n");
    }
}

fn cmd_tools() {
    let buf_addr = unsafe { core::ptr::addr_of!(BUF.bytes) as u64 };
    let result = unsafe { syscall_buf(8, buf_addr, MAX_ENTRIES as u64) };
    write_str("  SYSCALL  REQUIRED_RIGHTS  SIDE_EFFECTING\r\n");
    let count = core::cmp::min(result as usize, MAX_ENTRIES);
    for i in 0..count {
        let base = unsafe { buf_base().add(i * TOOL_DESCRIPTOR_SIZE) };
        let syscall_num = unsafe { read_u32_le(base, 0) };
        let required_rights = unsafe { read_u32_le(base, 4) };
        let side_effecting = unsafe { read_u32_le(base, 8) };
        write_str("  ");
        write_u64_dec(syscall_num as u64);
        write_str("        ");
        write_hex_u64(required_rights as u64);
        write_str("        ");
        write_str(if side_effecting != 0 { "yes" } else { "no" });
        write_str("\r\n");
    }
}

fn cmd_audit() {
    let buf_addr = unsafe { core::ptr::addr_of!(BUF.bytes) as u64 };
    let result = unsafe { syscall_buf(9, buf_addr, MAX_ENTRIES as u64) };
    if result == u64::MAX {
        write_str("permission denied: this shell was not granted Rights::AUDIT_QUERY\r\n");
        return;
    }
    if result == 0 {
        write_str("(no audit records attributed to this shell process yet)\r\n");
        return;
    }
    write_str("  SEQ  KIND  A  B\r\n");
    let count = core::cmp::min(result as usize, MAX_ENTRIES);
    for i in 0..count {
        let base = unsafe { buf_base().add(i * AUDIT_ENTRY_SIZE) };
        let seq = unsafe { read_u64_le(base, 0) };
        let kind = unsafe { read_u32_le(base, 8) };
        let a = unsafe { read_u32_le(base, 12) };
        let b = unsafe { read_u32_le(base, 16) };
        write_str("  ");
        write_u64_dec(seq);
        write_str("    ");
        write_u64_dec(kind as u64);
        write_str("    ");
        write_u64_dec(a as u64);
        write_str("    ");
        write_u64_dec(b as u64);
        write_str("\r\n");
    }
}

/// Parses a small hex literal ("64", "0x64") from `s`. Returns None on
/// anything that isn't a valid hex digit sequence.
fn parse_hex_u16(s: &[u8]) -> Option<u16> {
    let mut s = s;
    if s.len() >= 2 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
        s = &s[2..];
    }
    if s.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &c in s {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        v = v * 16 + d as u32;
        if v > 0xFFFF {
            return None;
        }
    }
    Some(v as u16)
}

/// Real, deliberate capability-boundary demo (see module doc): reads
/// ONE byte from an arbitrary port the caller names. This process's OWN
/// PortIoRange only covers COM1 (0x3F8-0x3FF) -- naming any other port
/// hits the real TSS IOPB and faults, killing this process. That IS the
/// real, hardware-enforced answer to "what happens when a shell command
/// exceeds its capabilities" -- not something this function can catch
/// or soften, by design.
fn cmd_rawin(arg: &[u8]) {
    match parse_hex_u16(arg) {
        Some(port) => {
            write_str("reading port ");
            write_hex_u64(port as u64);
            write_str(" (if this shell wasn't granted it, this process ends here -- real IOPB enforcement, not a message this code prints)\r\n");
            let value = unsafe { inb(port) };
            write_str("  value = ");
            write_hex_u64(value as u64);
            write_str("\r\n");
        }
        None => write_str("usage: rawin <hex port>, e.g. rawin 3f8\r\n"),
    }
}

fn cmd_help() {
    write_str("Agentic OS shell -- commands:\r\n");
    write_str("  help            this text\r\n");
    write_str("  ps              real typed thread list (syscall 7, Rights::INTROSPECT)\r\n");
    write_str("  tools           real typed tool/intent catalog (syscall 8, unconditional)\r\n");
    write_str("  audit           this process's OWN audit trail (syscall 9, Rights::AUDIT_QUERY)\r\n");
    write_str("  rawin <port>    read one byte from a raw port -- ONLY COM1 is granted;\r\n");
    write_str("                  anything else demonstrates real capability enforcement\r\n");
    write_str("                  by ending this process (see module doc)\r\n");
}

const LINE_MAX: usize = 128;

fn read_line(buf: &mut [u8; LINE_MAX]) -> usize {
    let mut len = 0usize;
    loop {
        let b = read_byte();
        if b == b'\r' || b == b'\n' {
            write_str("\r\n");
            return len;
        }
        if b == 0x08 || b == 0x7F {
            // backspace/DEL
            if len > 0 {
                len -= 1;
                write_str("\x08 \x08");
            }
            continue;
        }
        if len < LINE_MAX {
            buf[len] = b;
            len += 1;
            write_byte(b); // echo
        }
    }
}

fn split_first_word(line: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0;
    while i < line.len() && line[i] != b' ' {
        i += 1;
    }
    let word = &line[..i];
    let mut rest = i;
    while rest < line.len() && line[rest] == b' ' {
        rest += 1;
    }
    (word, &line[rest..])
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write_str("\r\nAgentic OS shell -- Phase 7 (docs/ROADMAP.md Sec5 Phase 7)\r\n");
    write_str("type 'help' for commands\r\n\r\n");

    let mut line = [0u8; LINE_MAX];
    loop {
        write_str("agentos> ");
        let len = read_line(&mut line);
        let (cmd, rest) = split_first_word(&line[..len]);
        match cmd {
            b"help" => cmd_help(),
            b"ps" => cmd_ps(),
            b"tools" => cmd_tools(),
            b"audit" => cmd_audit(),
            b"rawin" => cmd_rawin(rest),
            b"" => {}
            _ => write_str("unknown command -- type 'help'\r\n"),
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
