//! Phase 13 deliverable 2's own real, adversarial proof — closing the
//! gap between "the mechanism exists" (`installer.rs`) and "a real,
//! previously-unmodified ELF app was actually installed under it."
//! Reuses `user_rs/serial_driver` (already real and proven since Phase
//! 3 — see `user_driver.rs::spawn_serial_driver`) as the test subject,
//! spawned this time through `installer::install_into_current_thread`
//! instead of `user_driver.rs`'s own hand-written setup, with a
//! manifest that declares ONLY `PortIoRange` — the one capability this
//! specific app actually needs and uses (a real COM1 write). A second,
//! deliberately over-asked `Surface` request (this app never touches a
//! framebuffer) proves the installer's own manifest enforcement, not
//! just that the app happens to work.
#![cfg(feature = "installer_demo")]

use crate::capability::{KernelObjectKind, Rights};
use crate::installer::{self, CapRequest};
use crate::manifest::{CapKind, Manifest};
use crate::{gdt, klog_info, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

static SERIAL_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/serial_driver/target/x86_64-unknown-none/release/serial_driver");

/// This app's own declared manifest: "I need COM1's ports. Nothing
/// else." -- the honest in-kernel stand-in for a real installer parsing
/// this off disk (deliverable 2's own disclosed remaining gap, see
/// `installer.rs`'s module doc).
fn app_manifest() -> Manifest {
    Manifest::NONE.allow(CapKind::PortIoRange)
}

pub fn start() {
    thread::spawn(installer_demo_thread);
}

extern "C" fn installer_demo_thread() {
    let requests = [
        CapRequest {
            kind: CapKind::PortIoRange,
            object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
            rights: Rights::PORT_IO,
            label: "com1",
        },
        // Real, deliberate over-ask: this app's manifest never declared
        // Surface, and serial_driver's own code never asks for one --
        // this exists purely to prove the INSTALLER refuses an
        // undeclared kind, the exact adversarial shape Phase 13's own
        // exit criterion names ("an app that tries to use an
        // undeclared capability is denied").
        CapRequest {
            kind: CapKind::Surface,
            object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: 10, height: 10 },
            rights: Rights::MAP,
            label: "adversarial_surface",
        },
    ];

    klog_info!("INSTALLER_DEMO_START");
    let Some((entry, space)) = installer::install_into_current_thread(SERIAL_DRIVER_ELF, DRIVER_STACK_VADDR, app_manifest(), &requests) else {
        klog_info!("INSTALLER_DEMO_INSTALL_FAILED");
        return;
    };

    let kernel_stack_top = thread::current_kernel_stack_top();
    gdt::set_kernel_stack(kernel_stack_top);
    syscall::set_kernel_stack(kernel_stack_top);
    syscall::init();

    unsafe { vmm::switch_address_space(space) };
    thread::set_current_address_space(space);
    klog_info!("INSTALLER_DEMO_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + 4096);
    unsafe { ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096) };
}
