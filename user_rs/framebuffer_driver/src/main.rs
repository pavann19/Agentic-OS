//! Second real user-space driver (Phase 3): framebuffer. Same
//! standalone-ELF64 pattern as `user_rs/serial_driver`, loaded and
//! mapped by `kernel_rs/src/elf.rs`.
//!
//! What this proves: the kernel granted this process a real `MmioRegion`
//! capability for the actual GOP framebuffer `boot_rs` found at boot
//! (`driver.rs::map_mmio`, the same real per-page mapping mechanism
//! `iommu.rs`/`driver.rs` document elsewhere), mapped into THIS
//! process's own address space at a virtual address the kernel chose
//! (there is no way for a process to guess or reach kernel-internal
//! addresses -- it only sees what `map_mmio` handed back). This program
//! writes a real, recognizable pixel pattern directly into that mapped
//! memory, unmediated after the one-time grant, then signals readiness
//! via syscall 4 -- at which point `user_driver.rs`'s kernel-side verify
//! thread reads back the SAME physical framebuffer memory independently
//! (through the kernel's own MMIO window, not through anything this
//! process wrote to) and confirms the marker landed. That independent
//! kernel-side readback, not a log line this process prints about
//! itself, is what makes the proof real rather than self-reported.
//!
//! Geometry/base-vaddr handoff: there is no argument-passing syscall ABI
//! yet (docs/ROADMAP.md's "syscalls validate all user pointers" note in
//! syscall.rs applies to a FUTURE syscall that takes one, not to this),
//! so `user_driver.rs` writes a small, fixed-layout `FbInfo` struct into
//! a page at a well-known fixed virtual address before entering ring 3
//! -- a real, if minimal, boot-time ABI contract between the kernel and
//! this specific driver, analogous to how `BootInfo` itself is handed to
//! the kernel by the bootloader.

#![no_std]
#![no_main]

const FB_INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const FB_TEST_MARKER: u32 = 0xAABB_CCDD;
const MARKER_PIXEL_COUNT: usize = 64;

#[repr(C)]
struct FbInfo {
    base_vaddr: u64,
    width: u32,
    height: u32,
    pixels_per_scan_line: u32,
}

/// syscall(num=4, a0=token): framebuffer-driver-ready signal (syscall.rs).
unsafe fn syscall4(value: u64) {
    core::arch::asm!(
        "mov rax, 4",
        "syscall",
        in("rdi") value,
        lateout("rax") _,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(FB_INFO_VADDR as *const FbInfo);
        let fb = info.base_vaddr as *mut u32;
        // Real, recognizable pattern: the first MARKER_PIXEL_COUNT pixels
        // of row 0 set to a fixed, distinctive 32-bit value. Real
        // volatile writes into real mapped MMIO -- framebuffer memory is
        // mapped PAGE_CACHE_DISABLE (driver.rs::map_mmio), so these
        // writes are not just sitting in a CPU cache line pretending to
        // have landed.
        for i in 0..MARKER_PIXEL_COUNT.min((info.width as usize).max(1)) {
            core::ptr::write_volatile(fb.add(i), FB_TEST_MARKER);
        }
        syscall4(0xF6);
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
