//! Spawns the first REAL user-space driver (Phase 3's "first user-space
//! drivers" item): loads `user_rs/serial_driver`'s actual compiled ELF64
//! binary via `elf.rs`'s real loader, grants it a real `PortIoRange`
//! capability for COM1 through the exact same `driver.rs` mediation path
//! every other capability-gated hardware access in this kernel uses, and
//! enters ring 3 at the binary's own real entry point — not a hardcoded
//! address chosen by the kernel, the address the ELF header itself
//! states.
//!
//! Scope note (also in `user_rs/serial_driver/src/main.rs` and
//! `elf.rs`): only one real user-space driver exists yet (serial, since
//! it needed no MMIO/interrupt capability to prove the ELF-loading path
//! end to end, only port I/O); framebuffer and PS/2 keyboard — the other
//! two `docs/ROADMAP.md` names for this item — are not yet ported. The
//! ELF itself is embedded at kernel build time via `include_bytes!`,
//! since there is no filesystem yet (Phase 4) to load it from at
//! runtime; that's a real, honest interim source, not a simulated one —
//! see `elf.rs`'s doc comment.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

/// The real, compiled output of `user_rs/serial_driver` -- built
/// separately (its own crate, own target, own linker script; see that
/// crate's Cargo.toml/`.cargo/config.toml`), embedded here as raw bytes.
/// `elf.rs::load()` parses this exactly as it would parse bytes read
/// from a real disk later.
static SERIAL_DRIVER_ELF: &[u8] = include_bytes!(
    "../../user_rs/serial_driver/target/x86_64-unknown-none/release/serial_driver"
);

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

pub fn spawn_serial_driver() {
    thread::spawn(serial_driver_thread);
}

extern "C" fn serial_driver_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, SERIAL_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("USER_DRIVER_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DRIVER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real capability grant BEFORE this process ever reaches ring 3:
        // a dedicated, throwaway CapabilityTable exists only long enough
        // to mediate the one decision that matters -- opening exactly
        // COM1's 8 ports (0x3F8-0x3FF) in the TSS IOPB. Once
        // grant_port_access returns, the table itself is no longer
        // needed: the CPU enforces the IOPB in hardware from then on,
        // same as driver.rs's module doc states.
        let mut table = CapabilityTable::new();
        let cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, cap) {
            Ok(()) => klog_info!("USER_DRIVER_PORT_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("USER_DRIVER_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!(
            "USER_DRIVER_ELF_ENTER entry=0x{:x} stack=0x{:x}",
            entry,
            DRIVER_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
    }
}
