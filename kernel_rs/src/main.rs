//! Agentic OS kernel — Rust port, Phase 0.
//!
//! This is a deliberately narrow first slice, not the whole kernel. It ports
//! exactly the boot-critical path that already exists and is evidenced in
//! `kernel/kernel.c` / `kernel/klog.c` / `kernel/serial.c`: validate
//! `BootInfo`, bring up serial logging, emit `KERNEL_ENTER`, and halt.
//!
//! Everything else the C kernel currently does — GDT, IDT, PMM, paging,
//! graphics, keyboard — is NOT ported yet. See `PHASE0_PROGRESS.md` for
//! what's next and why this slice was cut here: the goal is a real,
//! QEMU-verified `KERNEL_ENTER` checkpoint from Rust code before porting
//! anything else, rather than a large unverified port landing all at once.
#![no_std]
#![no_main]

pub mod bootinfo;
pub mod klog;
pub mod serial;

use bootinfo::BootInfo;

#[no_mangle]
pub extern "sysv64" fn kernel_main(boot_info: *const BootInfo) -> ! {
    klog::init();

    let info = match unsafe { bootinfo::validate(boot_info) } {
        Ok(info) => info,
        Err(reason) => klog::panic(reason),
    };

    klog_info!("KERNEL_ENTER");
    klog_info!("BootInfo version={} size={}", info.version, info.size);
    klog_info!("Rust kernel slice: boot info validated, serial live.");
    klog_info!(
        "NOTE: GDT/IDT/PMM/paging/graphics not yet ported to Rust (Phase 0 in progress)."
    );

    // Nothing past this point exists yet on the Rust side — halt cleanly
    // rather than pretending the boot sequence continues.
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Best-effort: the message may not always render (no heap, no alloc),
    // but the location is always available and is the useful part.
    if let Some(location) = info.location() {
        klog_error!(
            "RUST PANIC at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    } else {
        klog_error!("RUST PANIC (no location info)");
    }
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
