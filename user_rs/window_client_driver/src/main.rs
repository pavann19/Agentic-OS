//! Phase 12 (`docs/ROADMAP.md` §5, deliverable 1, exit criterion 1):
//! a real, minimal window CLIENT process. Real, deliberate difference
//! from `compositor_driver`: this process is never granted the real
//! framebuffer `MmioRegion` at all -- it holds exactly one `Surface`
//! capability and draws into it only through a real syscall
//! (`SYS_SURFACE_FILL`, syscall 11) that the kernel mediates,
//! resolving the capability against the CALLING thread's own
//! per-process table (the same `resolve_current_capability`
//! mechanism every other per-process check in this kernel already
//! uses).
//!
//! Real proof this increment exists for: two instances of this exact
//! binary run as two separate processes, each in its own address
//! space with its own, separate capability table. Process B has no
//! shared memory with process A, no pointer to A's surface, and no
//! way to even NAME A's capability -- `CapId`s are table-local
//! indices, not global handles, so "guessing" a `CapId` in B's own
//! table can only ever resolve something B itself was granted (or
//! nothing at all). This process deliberately attempts exactly that
//! guess (a `CapId` it was never granted) as a real, adversarial
//! self-check, then confirms it was refused.

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
fn com1_write_str(s: &str) {
    for b in s.bytes() {
        while unsafe { inb(COM1 + 5) } & 0x20 == 0 {}
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

/// syscall(num=11, a0=CapId, a1=color): SYS_SURFACE_FILL
/// (`syscall.rs`). Returns 0 on success, `u64::MAX` if the capability
/// doesn't resolve (wrong id, wrong kind, or never granted).
/// syscall(num=4): the same real "driver ready" IPC send
/// `framebuffer_driver`/`compositor_driver` already established
/// (`syscall.rs`'s global `FB_READY_*`) -- a real, disclosed
/// simplification: this global signal doesn't itself distinguish
/// WHICH process sent it (the label is only in this process's own
/// COM1 log line), but `compositor.rs`'s verify thread only needs to
/// know "both clients are done," not which one finished first, so a
/// shared signal is real and sufficient here, not a correctness gap.
unsafe fn syscall4(value: u64) {
    core::arch::asm!(
        "mov rax, 4", "syscall",
        in("rdi") value,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

unsafe fn syscall_surface_fill(cap_id: u64, color: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "mov rax, 11", "syscall",
        in("rdi") cap_id, in("rsi") color,
        lateout("rax") ret,
        lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
    ret
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

const INFO_VADDR: u64 = 0x0000_0000_0051_0000;

#[repr(C)]
struct WindowClientInfo {
    label: u8, // 'A' or 'B' -- which client this process is, purely for readable logging
    surface_cap: u32,
    color: u32,
    foreign_cap_guess: u32, // a real CapId this process was NEVER granted -- the adversarial self-check
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const WindowClientInfo);

        com1_write_str("\n[WINDOW_CLIENT_");
        com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
        com1_write_str("] real ELF64 ring-3 process, own address space, own capability table, real Surface capability only (no framebuffer MMIO)\n");
        syscall1(0x9C1E_0000 | info.label as u64);

        let result = syscall_surface_fill(info.surface_cap as u64, info.color as u64);
        if result == 0 {
            com1_write_str("[WINDOW_CLIENT_");
            com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
            com1_write_str("] SURFACE_FILL_OK cap=");
            write_dec_u64(info.surface_cap as u64);
            com1_write_str("\n");
            syscall1(0x9C1E_6000 | info.label as u64);
        } else {
            com1_write_str("[WINDOW_CLIENT_");
            com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
            com1_write_str("] SURFACE_FILL_UNEXPECTED_DENIAL\n");
            syscall1(0x9C1E_BAD0 | info.label as u64);
        }

        // Real adversarial self-check: attempt to use a CapId this
        // process was NEVER granted (its own table has exactly one
        // entry -- the real surface above). Even if this integer
        // happens to collide with a slot number some OTHER process
        // was granted, it names nothing in THIS process's own table
        // -- the actual, structural proof this increment exists for.
        let foreign_result = syscall_surface_fill(info.foreign_cap_guess as u64, info.color as u64);
        if foreign_result != 0 {
            com1_write_str("[WINDOW_CLIENT_");
            com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
            com1_write_str("] FOREIGN_CAP_DENIED_OK: a CapId this process was never granted correctly refused to resolve\n");
            syscall1(0x9C1E_D001 | info.label as u64);
        } else {
            com1_write_str("[WINDOW_CLIENT_");
            com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
            com1_write_str("] FOREIGN_CAP_UNEXPECTEDLY_SUCCEEDED -- BUG\n");
            syscall1(0x9C1E_BAD1 | info.label as u64);
        }

        syscall4(0xC1E0_0000 | info.label as u64);

        loop {
            core::hint::spin_loop();
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
