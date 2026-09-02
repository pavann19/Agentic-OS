//! Phase 5's agent process model (`docs/ROADMAP.md` §5 Phase 5,
//! deliverable 1): "an ordinary user-space process holding a restricted
//! capability set, with no special kernel privileges." This is that
//! process — a real, freestanding ELF64 ring-3 binary, no different in
//! kind from `serial_driver`/`framebuffer_driver`/`keyboard_driver`,
//! except that it holds no hardware capability at all. Its only possible
//! capability is `Rights::INTROSPECT` over a Phase 5 `IntrospectionHandle`
//! (`kernel_rs/src/capability.rs`), and it never assumes it has one.
//!
//! `kernel_rs/src/agent.rs` spawns this EXACT SAME compiled binary
//! twice: once with that capability granted before the process ever
//! starts running (`thread::spawn_with_capability`), once without. This
//! file's code is completely capability-agnostic — it always attempts
//! the same syscall and reports whatever actually happens. The two
//! processes' different outcomes are entirely the kernel's own
//! capability check (`docs/ROADMAP.md`'s Phase 5 exit criterion: "a
//! policy denial is enforced by the kernel's capability check, not by
//! the agent's cooperation"), never a branch in this code.
//!
//! What "structured introspection, not text scraping" means here,
//! concretely: syscall 7 hands this process back real `ThreadInfo`
//! structs — `id`/`state`/`is_user` as actual typed fields at a fixed
//! byte layout — not a string it has to parse. This file decodes those
//! fields directly (`decode_entry` below) and logs each one through its
//! own typed integer fields, never by re-emitting or re-parsing a log
//! line.

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

/// Syscall 7: real structured introspection (`kernel_rs/src/syscall.rs`,
/// `kernel_rs/src/introspect.rs`). `buf` = this process's OWN buffer
/// (validated by the kernel against ITS OWN page tables before any
/// write happens — `vmm::validate_user_buffer_writable`), `max_entries`
/// = its capacity. Returns the real entry count written, or `u64::MAX`
/// if the calling process's own capability table doesn't hold
/// `Rights::INTROSPECT` — enforced kernel-side, not by this code
/// choosing not to ask.
unsafe fn syscall7_introspect(buf: u64, max_entries: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "mov rax, 7", "syscall",
        in("rdi") buf, in("rsi") max_entries,
        lateout("rax") ret, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
    ret
}

const MAX_ENTRIES: usize = 16;
const THREAD_INFO_SIZE: usize = 16;

#[repr(C, align(8))]
struct IntrospectBuf {
    bytes: [u8; MAX_ENTRIES * THREAD_INFO_SIZE],
}

static mut BUF: IntrospectBuf = IntrospectBuf {
    bytes: [0u8; MAX_ENTRIES * THREAD_INFO_SIZE],
};

/// Real typed decode of one `ThreadInfo` entry — `kernel_rs::introspect`'s
/// own real, documented C layout (id: u64 LE, state: u32 LE, is_user:
/// u32 LE), not a guess. Returns (id, state, is_user).
unsafe fn decode_entry(index: usize) -> (u64, u32, u32) {
    let base = (core::ptr::addr_of!(BUF.bytes) as *const u8).add(index * THREAD_INFO_SIZE);
    let mut id: u64 = 0;
    for i in 0..8 {
        id |= (core::ptr::read_volatile(base.add(i)) as u64) << (i * 8);
    }
    let mut state: u32 = 0;
    for i in 0..4 {
        state |= (core::ptr::read_volatile(base.add(8 + i)) as u32) << (i * 8);
    }
    let mut is_user: u32 = 0;
    for i in 0..4 {
        is_user |= (core::ptr::read_volatile(base.add(12 + i)) as u32) << (i * 8);
    }
    (id, state, is_user)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1_write_str("\n[AGENT_DEMO] real ELF64 ring-3 process, no hardware capability, attempting structured introspection\n");
    unsafe { syscall1(0xA6E0_0000) }; // "AGENT enter" marker

    let buf_addr = unsafe { core::ptr::addr_of!(BUF.bytes) as u64 };
    let result = unsafe { syscall7_introspect(buf_addr, MAX_ENTRIES as u64) };

    if result == u64::MAX {
        com1_write_str("[AGENT_DEMO] INTROSPECT_DENIED -- no Rights::INTROSPECT capability held\n");
        unsafe { syscall1(0xA6DE_0000) }; // "AGENT DEnied" marker
    } else {
        com1_write_str("[AGENT_DEMO] INTROSPECT_OK -- real typed ThreadInfo entries follow\n");
        unsafe { syscall1(0xA6_0A0000 | result) }; // "AGENT OK, count=result"
        let count = core::cmp::min(result as usize, MAX_ENTRIES);
        let mut i = 0;
        while i < count {
            let (id, state, is_user) = unsafe { decode_entry(i) };
            let packed = 0xA4_00_0000u64 | ((id & 0xFF) << 16) | (((state as u64) & 0xFF) << 8) | (is_user as u64 & 0xFF);
            unsafe { syscall1(packed) };
            i += 1;
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
