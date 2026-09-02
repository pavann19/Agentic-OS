//! Phase 7 — text shell (`docs/ROADMAP.md` §5 Phase 7). Spawns a real
//! ring-3 process (`user_rs/shell`) and grants it exactly three
//! capabilities before it ever runs: a `PortIoRange` for COM1 (its
//! terminal), and the SAME `Rights::INTROSPECT`/`Rights::AUDIT_QUERY`
//! Phase 5 already built for agent processes — this shell IS an agent
//! process in every structural sense this kernel has, just one with a
//! human typing at it over a real serial line instead of a synthesized
//! ELF running unattended. "Capability-aware, not privileged"
//! (deliverable 2) is true by construction: there is no separate
//! "shell" privilege tier anywhere in this kernel for this process to
//! hold.

use crate::capability::{self, CapabilityTable, KernelObjectKind, Rights};
use crate::{driver, elf, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

static SHELL_ELF: &[u8] = include_bytes!("../../user_rs/shell/target/x86_64-unknown-none/release/shell");

const SHELL_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

pub fn spawn_shell() {
    let introspect_object = capability::create_object(KernelObjectKind::IntrospectionHandle);
    let audit_object = capability::create_object(KernelObjectKind::AuditQueryHandle);

    klog_info!("SHELL_SPAWN_START");
    thread::spawn_with_capabilities(
        shell_thread,
        vmm::kernel_pml4_phys(),
        &[(introspect_object, Rights::INTROSPECT), (audit_object, Rights::AUDIT_QUERY)],
    );
}

extern "C" fn shell_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match elf::load(space, SHELL_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("SHELL_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            SHELL_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real PortIoRange for COM1 -- the shell's own terminal. Same
        // one-time TSS IOPB mediation every other driver crate's own
        // COM1 grant already uses.
        let mut table = CapabilityTable::new();
        let port_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, port_cap) {
            Ok(()) => klog_info!("SHELL_PORT_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("SHELL_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("SHELL_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, SHELL_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, SHELL_STACK_VADDR + 4096);
    }
}
