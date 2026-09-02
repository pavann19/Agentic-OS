//! Phase 5 — Agent Runtime Substrate (`docs/ROADMAP.md` §5 Phase 5).
//! Spawns TWO real ring-3 processes from the EXACT SAME compiled ELF64
//! binary (`user_rs/agent_demo`) — one granted a real `Rights::INTROSPECT`
//! capability over a Phase 5 `IntrospectionHandle` object before it ever
//! starts running, one left with an empty capability table. Neither
//! process's own code branches on which it is; the only thing that
//! differs is what the kernel's own capability check
//! (`thread::resolve_current_capability`, syscall 7 in `syscall.rs`)
//! allows each of them to do.
//!
//! This is the live demonstration behind three of Phase 5's four exit
//! criteria at once:
//!   - "An agent process enumerates the system ... entirely through
//!     typed interfaces" — the authorized process's real `ThreadInfo`
//!     structs, decoded and logged with typed fields
//!     (`AGENT_ENTRY id=.. state=.. is_user=..`), never text-scraped.
//!   - "A policy denial is enforced by the kernel's capability check,
//!     not by the agent's cooperation" — the stranger process runs the
//!     IDENTICAL code and is refused by `CapabilityTable::resolve`
//!     itself, not by any check this file or `agent_demo` performs.
//!   - "A misbehaving agent is contained to its own process and its
//!     granted capabilities" — demonstrated adversarially: the stranger
//!     process's attempt to reach data it was never granted access to
//!     fails cleanly, the rest of the system (including the authorized
//!     agent) unaffected.
//! The fourth ("every agent action ... reconstructible from the audit
//! log alone") is already true by construction: `CapabilityTable::
//! resolve` audits every denial on the same path this syscall goes
//! through (`capability.rs`), same as every other capability check in
//! this kernel.

use crate::capability::{self, KernelObjectKind, Rights};
use crate::{elf, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

static AGENT_DEMO_ELF: &[u8] =
    include_bytes!("../../user_rs/agent_demo/target/x86_64-unknown-none/release/agent_demo");

const AGENT_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

/// Spawns both agent processes. Called once from `main.rs`, after the
/// capability/audit substrate (Phase 2) and the driver framework (Phase
/// 3) are both live — an agent process is an ordinary user-space
/// process in every respect this kernel already provides; nothing about
/// Phase 5 required new kernel primitives beyond the capability itself
/// and the syscall that checks it.
pub fn spawn_agent_demo() {
    let object_id = capability::create_object(KernelObjectKind::IntrospectionHandle);

    klog_info!("AGENT_AUTHORIZED_SPAWN_START");
    thread::spawn_with_capability(agent_authorized_thread, vmm::kernel_pml4_phys(), object_id, Rights::INTROSPECT);

    klog_info!("AGENT_STRANGER_SPAWN_START");
    thread::spawn(agent_stranger_thread);
}

/// Both entry points below run the IDENTICAL ELF -- the only difference
/// is which one was spawned via `spawn_with_capability` (see
/// `spawn_agent_demo`). Two separate `extern "C" fn`s exist only so their
/// own boot-log lines say which is which for a human reading the
/// evidence; the loaded process itself has no idea which one it is.
extern "C" fn agent_authorized_thread() {
    run_agent_demo("AGENT_AUTHORIZED");
}

extern "C" fn agent_stranger_thread() {
    run_agent_demo("AGENT_STRANGER");
}

fn run_agent_demo(label: &str) {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match elf::load(space, AGENT_DEMO_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("{}_ELF_LOAD_FAILED {:?}", label, e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            AGENT_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("{}_ELF_ENTER entry=0x{:x} stack=0x{:x}", label, entry, AGENT_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, AGENT_STACK_VADDR + 4096);
    }
}
