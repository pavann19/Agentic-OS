//! Real GUI mouse support: PS/2 mouse driver (IRQ12, the 8042's
//! auxiliary port), same real capability-gated ring-3 driver-process
//! discipline as `user_driver.rs`'s keyboard driver -- a real
//! `InterruptLine` capability for IRQ12 (syscalls 20/21, mirroring
//! syscalls 5/6's own keyboard-only mechanism) plus a real
//! `PortIoRange` grant for the shared 8042 ports (0x60/0x64, the SAME
//! physical ports the keyboard driver ALSO holds -- a real, correct,
//! per-process IOPB grant each, no conflict: PortIoRange access is a
//! per-process bitmap flag, never an exclusive lock).
//!
//! Real, disclosed scope: relative-motion PS/2 mouse only (the
//! standard 3-byte packet: button state + signed dx/dy), no scroll
//! wheel (IntelliMouse 4-byte packets), no USB HID mouse (Phase 11's
//! own already-disclosed gap applies here too).

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

static MOUSE_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/mouse_driver/target/x86_64-unknown-none/release/mouse_driver");

const MOUSE_DRIVER_STACK_VADDR: u64 = 0x0000_0000_0075_0000;

pub fn spawn() {
    thread::spawn(mouse_driver_thread);
}

extern "C" fn mouse_driver_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, MOUSE_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("MOUSE_DRIVER_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            MOUSE_DRIVER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real PortIoRange grant for the shared 8042 ports -- same real
        // range the keyboard driver also holds (see this module's own
        // doc for why that's correct, not a conflict).
        let mut port_table = CapabilityTable::new();
        let port_cap = driver::create_port_capability(&mut port_table, 0x60, 5, Rights::PORT_IO);
        match driver::grant_port_access(&port_table, port_cap) {
            Ok(()) => klog_info!("MOUSE_DRIVER_PORT_GRANTED base=0x60 count=5"),
            Err(e) => {
                klog_info!("MOUSE_DRIVER_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        let mut com1_table = CapabilityTable::new();
        let com1_cap = driver::create_port_capability(&mut com1_table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&com1_table, com1_cap) {
            Ok(()) => klog_info!("MOUSE_DRIVER_COM1_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("MOUSE_DRIVER_COM1_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // Real InterruptLine capability for the real, unmasked IRQ12
        // (pic.rs::MOUSE_VECTOR / idt.rs::h_mouse) -- syscalls 20/21
        // reuse this exact dedicated table on every subsequent
        // wait/ack call, mirroring syscall::init_kbd_capability.
        syscall::init_mouse_capability();

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("MOUSE_DRIVER_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, MOUSE_DRIVER_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, MOUSE_DRIVER_STACK_VADDR + 4096);
    }
}
