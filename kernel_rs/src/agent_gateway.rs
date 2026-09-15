//! Live Agent Bridge Gateway (`docs/TASK_LIVE_AGENT_BRIDGE.md`).
//!
//! Spawns a dedicated ring-3 `agent_gateway` process wired to COM2 (`0x2F8`).
//! Enables an external LLM session or host-side tool to query active GUI windows
//! and dispatch typed UI actions via `SYS_INTROSPECT_WINDOWS` (syscall 32)
//! and `SYS_AGENT_UI_ACTION` (syscall 33) through a typed wire protocol over COM2.
//!
//! Zero ambient authority: The gateway process is granted only COM2 PortIoRange
//! and an explicit IntrospectionHandle capability. Syscall permissions are
//! strictly verified by the kernel.

use crate::capability::{self, CapabilityTable, KernelObjectKind, Rights};
use crate::driver;
use crate::elf;
use crate::gdt;
use crate::klog_info;
use crate::pmm;
use crate::ring3;
use crate::serial;
use crate::syscall;
use crate::thread;
use crate::vmm;

static AGENT_GATEWAY_ELF: &[u8] =
    include_bytes!("../../user_rs/agent_gateway/target/x86_64-unknown-none/release/agent_gateway");

const AGENT_GATEWAY_STACK_VADDR: u64 = 0x0078_0000;

pub fn spawn_if_present() {
    // 1. Hardware presence check: probe UART COM2 scratch register
    if !serial::probe(serial::COM2) {
        klog_info!("COM2_NOT_PRESENT (agent bridge idle)");
        return;
    }

    klog_info!("COM2_FOUND base=0x2f8; initializing COM2 UART for Live Agent Bridge");
    serial::init(serial::COM2);

    #[cfg(not(feature = "agent_gateway_unauthorized"))]
    {
        let introspect_object = capability::create_object(KernelObjectKind::IntrospectionHandle);
        klog_info!("AGENT_GATEWAY_SPAWN: granting IntrospectionHandle Rights::INTROSPECT in slot 0");
        thread::spawn_with_capabilities(
            agent_gateway_thread,
            vmm::kernel_pml4_phys(),
            &[(introspect_object, Rights::INTROSPECT)],
        );
    }

    #[cfg(feature = "agent_gateway_unauthorized")]
    {
        klog_info!("AGENT_GATEWAY_SPAWN (UNAUTHORIZED ADVERSARIAL): spawning without Rights::INTROSPECT");
        thread::spawn(agent_gateway_thread);
    }
}

extern "C" fn agent_gateway_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match elf::load(space, AGENT_GATEWAY_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("AGENT_GATEWAY_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            AGENT_GATEWAY_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Grant COM1 (0x3F8) for debug logs
        let mut com1_table = CapabilityTable::new();
        let com1_cap = driver::create_port_capability(&mut com1_table, serial::COM1, 8, Rights::PORT_IO);
        if let Err(e) = driver::grant_port_access(&com1_table, com1_cap) {
            klog_info!("AGENT_GATEWAY_COM1_GRANT_FAILED {:?}", e);
            return;
        }

        // Grant COM2 (0x2F8) for bridge communication
        let mut com2_table = CapabilityTable::new();
        let com2_cap = driver::create_port_capability(&mut com2_table, serial::COM2, 8, Rights::PORT_IO);
        if let Err(e) = driver::grant_port_access(&com2_table, com2_cap) {
            klog_info!("AGENT_GATEWAY_COM2_GRANT_FAILED {:?}", e);
            return;
        }
        klog_info!("AGENT_GATEWAY_PORTS_GRANTED com1=0x3f8 com2=0x2f8");

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!(
            "AGENT_GATEWAY_ELF_ENTER entry=0x{:x} stack=0x{:x}",
            entry,
            AGENT_GATEWAY_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(entry, AGENT_GATEWAY_STACK_VADDR + 4096);
    }
}
