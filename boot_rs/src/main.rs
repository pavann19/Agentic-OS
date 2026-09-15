//! Agentic OS bootloader — Rust port.
//!
//! Loads kernel.elf and font.psf from the ESP root volume, maps the
//! kernel's PT_LOAD segments to their exact physical addresses, finds the
//! GOP framebuffer, builds BootInfo, exits boot services, and jumps to the
//! kernel entry point. Font rendering and GOP framebuffer use itself live
//! in the kernel, not here — this only has to load and hand them off
//! correctly.
#![no_std]
#![no_main]

mod bootinfo;
mod bootstrap_paging;
mod elf;
mod loader;
mod serial;
mod uefi;

use core::panic::PanicInfo;
use uefi::{Handle, Status, SystemTable, EFI_SUCCESS};

#[no_mangle]
pub extern "efiapi" fn efi_main(image_handle: Handle, system_table: *mut SystemTable) -> Status {
    serial::init();
    serial::write_str("BOOT_START\n");

    let st = unsafe { &*system_table };
    uefi::print(st.con_out, "Agentic OS Bootloader (Rust) loading...\r\n");

    let prepared = match unsafe { loader::prepare_boot(st, image_handle) } {
        Ok(p) => p,
        Err(loader::LoadError::Uefi(msg)) => {
            serial::write_str("BOOT_ERROR: ");
            serial::write_str(msg);
            serial::write_str("\n");
            loop {
                unsafe { core::arch::asm!("pause", options(nomem, nostack)) };
            }
        }
    };

    // Boot services are gone at this point — no more UEFI calls, no more
    // ConOut. Serial is still fine: it's raw port I/O, not a firmware
    // service.
    serial::write_str("EXIT_BOOT_SERVICES_OK\n");
    serial::write_str("KERNEL_ENTER\n");

    let kernel_entry: extern "sysv64" fn(*const bootinfo::BootInfo) -> ! =
        unsafe { core::mem::transmute(prepared.kernel_entry as usize) };
    kernel_entry(prepared.boot_info as *const bootinfo::BootInfo);

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
