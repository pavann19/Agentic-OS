//! Phase 5's agent process model (`docs/ROADMAP.md` §5 Phase 5,
//! deliverable 1): "an ordinary user-space process holding a restricted
//! capability set, with no special kernel privileges." This is that
//! process — a real, freestanding ELF64 ring-3 binary, no different in
//! kind from `serial_driver`/`framebuffer_driver`/`keyboard_driver`,
//! except that it holds no hardware capability at all. Its only
//! possible capabilities are `Rights::INTROSPECT` and
//! `Rights::AUDIT_QUERY` over the two Phase 5 handle objects
//! (`kernel_rs/src/capability.rs`), and it never assumes it has either.
//!
//! `kernel_rs/src/agent.rs` spawns this EXACT SAME compiled binary
//! three times, with three different capability outcomes decided
//! entirely kernel-side (full grants, none, or a policy-refused grant)
//! — this file's code is completely capability-agnostic. It always
//! attempts the same three syscalls in order and reports whatever
//! actually happens; the different outcomes across the three spawned
//! processes are entirely the kernel's own capability check and policy
//! engine (`docs/ROADMAP.md`'s Phase 5 exit criterion: "a policy denial
//! is enforced by the kernel's capability check, not by the agent's
//! cooperation"), never a branch in this code.
//!
//! Three real, typed exchanges happen here, none of them text:
//!   1. Syscall 8 — tool/intent discovery: real `ToolDescriptor`
//!      structs (`kernel_rs::tools`), unconditionally readable, telling
//!      this process what operations exist and what each would require
//!      BEFORE it has to have any of that hardcoded.
//!   2. Syscall 7 — structured introspection: real `ThreadInfo` structs,
//!      capability-gated.
//!   3. Syscall 9 — capability-scoped audit query: real `AuditEntryInfo`
//!      structs, this process's OWN audit trail only, capability-gated.
//! Every decode below reads fixed, documented byte offsets directly —
//! never a string this process has to parse.

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

fn com1_write_str(s: &str) {
    for b in s.bytes() {
        while !com1_tx_ready() {}
        unsafe { outb(COM1, b) };
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

/// Shared shape for every "num=X, buf, max_entries -> count" syscall
/// below (7, 8, 9) — same real args-in-rdi/rsi, result-in-rax pattern,
/// just parameterized on the syscall number so it isn't repeated three
/// times identically.
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

// One shared, generously-sized buffer for all three syscalls (used one
// at a time, never concurrently) -- real `#[repr(C, align(8))]` so a
// natural 8-byte alignment holds regardless of which fixed-layout
// struct is currently being decoded out of it.
const BUF_BYTES: usize = MAX_ENTRIES * AUDIT_ENTRY_SIZE; // the largest of the three entry sizes

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

/// Real typed decode of one `ThreadInfo` entry — `kernel_rs::introspect`'s
/// own documented C layout (id: u64 LE, state: u32 LE, is_user: u32 LE).
unsafe fn decode_thread_info(index: usize) -> (u64, u32, u32) {
    let base = buf_base().add(index * THREAD_INFO_SIZE);
    (read_u64_le(base, 0), read_u32_le(base, 8), read_u32_le(base, 12))
}

/// Real typed decode of one `ToolDescriptor` entry —
/// `kernel_rs::tools`'s documented layout (syscall_num/required_rights/
/// side_effecting, each u32 LE).
unsafe fn decode_tool_descriptor(index: usize) -> (u32, u32, u32) {
    let base = buf_base().add(index * TOOL_DESCRIPTOR_SIZE);
    (read_u32_le(base, 0), read_u32_le(base, 4), read_u32_le(base, 8))
}

/// Real typed decode of one `AuditEntryInfo` entry —
/// `kernel_rs::introspect`'s documented layout (seq: u64 LE, kind/a/b
/// each u32 LE).
unsafe fn decode_audit_entry(index: usize) -> (u64, u32, u32, u32) {
    let base = buf_base().add(index * AUDIT_ENTRY_SIZE);
    (read_u64_le(base, 0), read_u32_le(base, 8), read_u32_le(base, 12), read_u32_le(base, 16))
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1_write_str("\n[AGENT_DEMO] real ELF64 ring-3 process, capability set decided entirely by the kernel\n");
    unsafe { syscall1(0xA6E0_0000) }; // "AGENT enter" marker

    // 1. Tool/intent discovery (syscall 8) -- unconditional, no
    // capability required to see WHAT exists, only to use it.
    let buf_addr = unsafe { core::ptr::addr_of!(BUF.bytes) as u64 };
    let tool_count = unsafe { syscall_buf(8, buf_addr, MAX_ENTRIES as u64) };
    com1_write_str("[AGENT_DEMO] TOOLS_DISCOVERED -- real typed ToolDescriptor entries follow\n");
    unsafe { syscall1(0xA8_0A0000 | tool_count) };
    let ntools = core::cmp::min(tool_count as usize, MAX_ENTRIES);
    for i in 0..ntools {
        let (syscall_num, required_rights, side_effecting) = unsafe { decode_tool_descriptor(i) };
        let packed = 0xA8_000000u64
            | ((syscall_num as u64 & 0xFF) << 16)
            | ((required_rights as u64 & 0xFF) << 8)
            | (side_effecting as u64 & 0xFF);
        unsafe { syscall1(packed) };
    }

    // 2. Structured introspection (syscall 7) -- capability-gated.
    let result = unsafe { syscall_buf(7, buf_addr, MAX_ENTRIES as u64) };
    if result == u64::MAX {
        com1_write_str("[AGENT_DEMO] INTROSPECT_DENIED -- no Rights::INTROSPECT capability held\n");
        unsafe { syscall1(0xA6DE_0000) }; // "AGENT DEnied" marker
    } else {
        com1_write_str("[AGENT_DEMO] INTROSPECT_OK -- real typed ThreadInfo entries follow\n");
        unsafe { syscall1(0xA6_0A0000 | result) };
        let count = core::cmp::min(result as usize, MAX_ENTRIES);
        for i in 0..count {
            let (id, state, is_user) = unsafe { decode_thread_info(i) };
            let packed = 0xA4_00_0000u64 | ((id & 0xFF) << 16) | (((state as u64) & 0xFF) << 8) | (is_user as u64 & 0xFF);
            unsafe { syscall1(packed) };
        }
    }

    // 3. Capability-scoped audit query (syscall 9) -- capability-gated,
    // returns exactly THIS process's own audit trail.
    let audit_result = unsafe { syscall_buf(9, buf_addr, MAX_ENTRIES as u64) };
    if audit_result == u64::MAX {
        com1_write_str("[AGENT_DEMO] AUDIT_QUERY_DENIED -- no Rights::AUDIT_QUERY capability held\n");
        unsafe { syscall1(0xA9DE_0000) }; // "AGENT audit-query DEnied" marker
    } else {
        com1_write_str("[AGENT_DEMO] AUDIT_QUERY_OK -- real typed, capability-scoped AuditEntryInfo entries follow\n");
        unsafe { syscall1(0xA9_0A0000 | audit_result) };
        let count = core::cmp::min(audit_result as usize, MAX_ENTRIES);
        for i in 0..count {
            let (_seq, kind, a, b) = unsafe { decode_audit_entry(i) };
            let packed = 0xA9_000000u64 | ((kind as u64 & 0xFF) << 16) | ((a as u64 & 0xF) << 8) | (b as u64 & 0xFF);
            unsafe { syscall1(packed) };
        }
    }

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
