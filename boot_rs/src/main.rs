//! Agentic OS bootloader — Rust port, first slice.
//!
//! This is NOT yet a port of `boot/main.c`. It proves the native (no-Docker)
//! Rust UEFI pipeline end-to-end: build with `x86_64-unknown-uefi`, boot in
//! QEMU/OVMF, emit `BOOT_START` on serial and a visible line on the UEFI
//! console. ELF-kernel loading, font/memory-map handling, and the BootInfo
//! handoff are NOT implemented here yet — seem BOOT0_PROGRESS.md for what's
//! next and why this was cut here first (same reasoning as kernel_rs/'s
//! first slice: verify the toolchain and boot path before porting logic).
#![no_std]
#![no_main]

mod serial;
mod uefi;

use core::panic::PanicInfo;
use uefi::{Handle, Status, SystemTable, EFI_SUCCESS};

#[no_mangle]
pub extern "efiapi" fn efi_main(_image_handle: Handle, system_table: *mut SystemTable) -> Status {
    serial::init();
    serial::write_str("BOOT_START\n");

    let st = unsafe { &*system_table };
    uefi::print(st.con_out, "Agentic OS Bootloader (Rust) loading...\r\n");

    serial::write_str("BOOT_RS_HELLO_OK\n");

    // Nothing past this point exists yet — spin rather than pretend the boot
    // sequence continues. Returning EFI_SUCCESS here would hand control back
    // to the UEFI shell/BDS, which isn't the right behavior once this is a
    // real bootloader, but is harmless for this verification slice.
    loop {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack));
        }
    }

    #[allow(unreachable_code)]
    EFI_SUCCESS
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial::write_str("BOOT_RS_PANIC\n");
    loop {
        unsafe {
            core::arch::asm!("pause", options(nomem, nostack));
        }
    }
}
