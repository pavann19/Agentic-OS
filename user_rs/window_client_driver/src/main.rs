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

/// syscall(num=16, a0=CapId): SYS_SURFACE_PRESENT (`syscall.rs`). Real,
/// disclosed latency fix shared with `terminal_emulator`: `SYS_SURFACE_
/// FILL`/`SYS_SURFACE_DRAW_TEXT` now only write into this window's own
/// in-memory buffer -- nothing reaches the real framebuffer until this
/// is called once, after the whole fill+text batch below.
unsafe fn syscall_present(cap_id: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "mov rax, 16", "syscall",
        in("rdi") cap_id,
        // Real, disclosed bug found and fixed testing this exact
        // change: `a1` (rsi) selects full vs. partial present in the
        // kernel's own dispatch (0 = full) -- `lateout("rsi") _` alone
        // (this function's original form) never actually PASSES a
        // value in, it only marks the register clobbered, so `a1`
        // arrived as whatever garbage was already in rsi, which the
        // kernel then (usually) misread as a bogus partial-present
        // request instead of the intended full one. `in("rsi") 0u64`
        // is what actually zeroes it.
        in("rsi") 0u64,
        lateout("rax") ret,
        lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
    ret
}

/// syscall(num=12, a0=CapId): SYS_IPC_TRY_RECEIVE (`syscall.rs`).
/// Non-blocking -- returns u64::MAX immediately if nothing has been
/// routed to this process's own input endpoint yet, real evidence a
/// window that never holds focus can poll this forever without ever
/// hanging (Phase 12 exit criterion 4's own adversarial check: the
/// UNFOCUSED window must genuinely never receive anything, not just
/// "hasn't yet").
unsafe fn syscall_try_receive(cap_id: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "mov rax, 12", "syscall",
        in("rdi") cap_id,
        lateout("rax") ret,
        lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
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
    input_cap: u32, // this process's own real IpcEndpoint CapId for routed keyboard input
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

        // Phase 12 deliverable 4: real, minimal UI toolkit exercise --
        // draw a real two-line block of PSF1 text into this process's
        // OWN surface via agentic_sdk::text_widget (which itself calls
        // SYS_SURFACE_DRAW_TEXT, syscall 14), at the surface's top-left
        // corner -- deliberately away from the center pixel
        // compositor.rs's own verify thread already samples for the
        // solid-fill self-check above, so the two checks never collide.
        let mut text_region_mu = core::mem::MaybeUninit::<agentic_sdk::text_widget::TextRegion<2, 16>>::uninit();
        let text_region_ptr = text_region_mu.as_mut_ptr();
        agentic_sdk::text_widget::TextRegion::init_in_place(text_region_ptr);
        (*text_region_ptr).push_line(b"HI");
        (*text_region_ptr).push_line(b"OK");
        (*text_region_ptr).render(info.surface_cap, 16, 0x00FFFFFF, info.color);
        com1_write_str("[WINDOW_CLIENT_");
        com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
        com1_write_str("] TEXT_DRAWN via SYS_SURFACE_DRAW_TEXT\n");

        // UI icon exercise: draw a real 16x16 1-bit monochrome icon via
        // agentic_sdk::icon (SYS_SURFACE_DRAW_BITMAP, syscall 23)
        let icon = if info.label == b'A' {
            &agentic_sdk::icon::APP_ICON
        } else {
            &agentic_sdk::icon::TERMINAL_ICON
        };
        icon.draw(info.surface_cap, 32, 16, 0x00FFFFFF, 0);
        com1_write_str("[WINDOW_CLIENT_");
        com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
        com1_write_str("] BITMAP_DRAWN via SYS_SURFACE_DRAW_BITMAP\n");

        // Real, disclosed latency fix shared with terminal_emulator:
        // the fill above plus both push_line/render draw_text calls
        // only touched this window's own in-memory buffer -- present
        // exactly once now that the whole startup batch is done.
        syscall_present(info.surface_cap as u64);

        syscall4(0xC1E0_0000 | info.label as u64);

        // Phase 12 exit criterion 4: real, UNBOUNDED poll for routed
        // keyboard input on this process's OWN endpoint, for the rest
        // of this process's life. Real, disclosed reason it's
        // unbounded rather than a fixed retry count: a real human (or
        // this project's own HMP-injected synthetic keystroke) can
        // press a key at any real wall-clock time after boot, not
        // within some fixed early window -- a window that gave up
        // polling too early would silently miss it. This is safe to
        // leave unbounded specifically BECAUSE the sender
        // (`input_routing::deliver_key_event`) uses `ipc::try_send`
        // (fire-and-forget, never blocks) rather than the blocking
        // `ipc::send` -- an earlier real bug, found and fixed, where a
        // bounded poll racing a late keystroke deadlocked the ENTIRE
        // keyboard driver (see `ipc::try_send`'s own doc for the full
        // story). A window that never holds focus (this run's B) polls
        // forever and simply never receives anything -- real, expected,
        // harmless (this thread's own spin never blocks any other
        // thread), and the actual adversarial evidence this exit
        // criterion exists to demonstrate.
        loop {
            let r = syscall_try_receive(info.input_cap as u64);
            if r != u64::MAX {
                com1_write_str("[WINDOW_CLIENT_");
                com1_write_str(core::str::from_utf8(core::slice::from_ref(&info.label)).unwrap_or("?"));
                com1_write_str("] INPUT_EVENT_RECEIVED scancode=0x");
                write_dec_u64(r);
                com1_write_str("\n");
                syscall1(0x1E9E_0000 | info.label as u64);
            }
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
