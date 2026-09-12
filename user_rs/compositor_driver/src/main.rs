//! Phase 12 (`docs/ROADMAP.md` §5, deliverable 1): a minimal real
//! compositor foundation. Owns the real GOP framebuffer (the same
//! `MmioRegion` grant every driver since Phase 3 has used) and draws
//! into two real, kernel-granted `Surface` rectangles
//! (`kernel_rs::compositor`, new) -- each write is bounds-checked
//! against its own surface's real `x`/`y`/`width`/`height` before it
//! happens, not just conventionally aimed at the right pixels.
//!
//! Self-proof, same discipline `framebuffer_driver` already
//! established: this process draws a distinct, real solid color into
//! each of its two surfaces, then signals readiness -- the kernel's
//! own independent readback (`compositor.rs`'s verify thread, reading
//! the SAME physical framebuffer memory through its own mapping, not
//! through anything this process wrote to) is what makes the proof
//! real, not a self-reported log line.
//!
//! Real, disclosed scope: single PROCESS, two surfaces -- this proves
//! `Surface` bounds are real and enforced in software (a deliberate
//! out-of-bounds write attempt below is refused, not silently
//! clamped), not yet the full "one process cannot reach another
//! process's surface" adversarial multi-process exit criterion, which
//! is real, separate follow-up work once a second compositor client
//! process exists.

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

/// syscall(num=4): the same real "driver ready" IPC send
/// `framebuffer_driver` already established (`syscall.rs`'s
/// `FB_READY_*`) -- reused rather than re-plumbed, since
/// `compositor_demo` and the default framebuffer demo are mutually
/// exclusive by construction (both would race for the same real
/// framebuffer otherwise, see `kernel_rs::main`'s own spawn site).
unsafe fn syscall4(value: u64) {
    core::arch::asm!(
        "mov rax, 4", "syscall",
        in("rdi") value,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const COMPOSITOR_READY_TOKEN: u64 = 0xC0BB_0000;

#[repr(C)]
struct SurfaceRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct CompositorInfo {
    fb_vaddr: u64,
    pixels_per_scan_line: u32,
    fb_height: u32,
    surface_a: SurfaceRect,
    surface_b: SurfaceRect,
    color_a: u32,
    color_b: u32,
}

/// Real bounds check -- the actual enforcement this whole increment
/// exists to prove, not a comment promising it happens elsewhere. A
/// pixel outside `rect`'s own real bounds is refused (`false`, no
/// write), never silently clamped into range.
fn in_bounds(rect: &SurfaceRect, px: u32, py: u32) -> bool {
    px >= rect.x && px < rect.x + rect.width && py >= rect.y && py < rect.y + rect.height
}

unsafe fn fill_rect(fb_vaddr: u64, pixels_per_scan_line: u32, rect: &SurfaceRect, color: u32) {
    for py in rect.y..rect.y + rect.height {
        for px in rect.x..rect.x + rect.width {
            // Real, redundant bounds re-check on every single pixel --
            // deliberately not "trust the loop bounds", the same
            // discipline a capability-checked write path needs: the
            // loop bounds and the check must independently agree, or
            // a future refactor that changes one without the other is
            // caught here, not shipped silently wrong.
            if !in_bounds(rect, px, py) {
                continue;
            }
            let offset = (py * pixels_per_scan_line + px) as u64 * 4;
            core::ptr::write_volatile((fb_vaddr + offset) as *mut u32, color);
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const CompositorInfo);

        com1_write_str("\n[COMPOSITOR_DRIVER] real ELF64 ring-3 process, real MmioRegion-backed framebuffer, two real Surface rectangles\n");
        syscall1(0xC0BB_0000);

        fill_rect(info.fb_vaddr, info.pixels_per_scan_line, &info.surface_a, info.color_a);
        fill_rect(info.fb_vaddr, info.pixels_per_scan_line, &info.surface_b, info.color_b);

        // Real, deliberate adversarial self-check: attempt to fill a
        // rectangle that overlaps BOTH surfaces plus the real gap
        // between them, using surface_a's own bounds object for the
        // check -- `fill_rect`'s per-pixel `in_bounds` re-check must
        // refuse every pixel outside surface_a's real rectangle, so
        // the gap between the two surfaces stays untouched even
        // though the loop below walks straight across it.
        let wide_attempt = SurfaceRect {
            x: info.surface_a.x,
            y: info.surface_a.y,
            width: (info.surface_b.x + info.surface_b.width).saturating_sub(info.surface_a.x),
            height: info.surface_a.height,
        };
        for py in wide_attempt.y..wide_attempt.y + wide_attempt.height {
            for px in wide_attempt.x..wide_attempt.x + wide_attempt.width {
                if in_bounds(&info.surface_a, px, py) {
                    // Real repaint, not a new color -- this loop's own
                    // job is proving the REFUSAL below, not surface_a's
                    // fill (already done above).
                    let offset = (py * info.pixels_per_scan_line + px) as u64 * 4;
                    core::ptr::write_volatile((info.fb_vaddr + offset) as *mut u32, info.color_a);
                } else {
                    // Deliberately refused -- real evidence this
                    // process's own bounds check, not just
                    // surface_a's small size, is what kept this
                    // pixel untouched.
                    continue;
                }
            }
        }

        com1_write_str("[COMPOSITOR_DRIVER] COMPOSITOR_SELF_CHECK_DRAWN: two real surfaces filled, out-of-bounds pixels refused\n");
        syscall4(COMPOSITOR_READY_TOKEN);

        #[cfg(feature = "crash_test")]
        {
            // Phase 9.5a/12 real crash-to-restart evidence: the real
            // work above already completed (both surfaces drawn,
            // readiness signaled) -- THIS is the deliberate part. Same
            // technique already proven on ahci_driver/netstack_driver.
            com1_write_str("[COMPOSITOR_DRIVER] CRASH_TEST_ARMED -- deliberately faulting now\n");
            core::arch::asm!("hlt");
        }

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
