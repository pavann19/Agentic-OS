//! Phase 5 — Agent Runtime Substrate (`docs/ROADMAP.md` §5 Phase 5).
//! Spawns THREE real ring-3 processes from the EXACT SAME compiled
//! ELF64 binary (`user_rs/agent_demo`):
//!   - `AGENT_AUTHORIZED`: granted real `Rights::INTROSPECT` AND
//!     `Rights::AUDIT_QUERY` capabilities, both minted before it ever
//!     starts running.
//!   - `AGENT_STRANGER`: granted nothing at all — an empty capability
//!     table.
//!   - `AGENT_POLICY_VIOLATOR`: attempts a grant of `Rights::PORT_IO` —
//!     a right no agent process is allowed to hold under this kernel's
//!     Phase 5 policy (`policy.rs`) — refused at GRANT time, before the
//!     process even starts, distinct from the stranger's later USE-time
//!     refusal.
//! None of the three process's own code branches on which it is; the
//! only thing that differs between them is what the kernel's own
//! capability check and policy engine allow each to do.
//!
//! This is the live demonstration behind all four of Phase 5's exit
//! criteria:
//!   - "An agent process enumerates the system ... entirely through
//!     typed interfaces" — the authorized process's real `ThreadInfo`/
//!     `ToolDescriptor`/`AuditEntryInfo` structs, decoded and logged
//!     field by field, never text-scraped.
//!   - "A policy denial is enforced by the kernel's capability check,
//!     not by the agent's cooperation" — the stranger runs the
//!     IDENTICAL code and is refused by `CapabilityTable::resolve`
//!     itself; the policy violator is refused even earlier, by
//!     `policy::allows` inside `thread::spawn_with_capabilities`,
//!     before any syscall is even attempted.
//!   - "Every agent action ... reconstructible from the audit log
//!     alone" — `capability.rs::resolve`'s denial-audit fix (this same
//!     session) plus the new `actor_tid` field (`audit.rs`) make this
//!     concretely checkable: the authorized agent's OWN
//!     `AGENT_AUDIT_QUERY` syscall reads back exactly the records ITS
//!     OWN actions caused, nothing more, nothing less.
//!   - "A misbehaving agent is contained to its own process and its
//!     granted capabilities — demonstrated adversarially" — both the
//!     stranger and the policy violator demonstrate exactly this, two
//!     different ways (a resolve-time and a grant-time refusal), with
//!     the rest of the system (including the authorized agent)
//!     unaffected either time.

use crate::capability::{self, CapabilityTable, KernelObjectKind, Rights};
use crate::{driver, elf, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

static AGENT_DEMO_ELF: &[u8] =
    include_bytes!("../../user_rs/agent_demo/target/x86_64-unknown-none/release/agent_demo");

const AGENT_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

/// Spawns all three agent processes. Called once from `main.rs`, after
/// the capability/audit substrate (Phase 2) and the driver framework
/// (Phase 3) are both live — an agent process is an ordinary user-space
/// process in every respect this kernel already provides; nothing about
/// Phase 5 required new kernel primitives beyond the capabilities
/// themselves, the syscalls that check them, and the policy engine that
/// gates what gets granted in the first place.
pub fn spawn_agent_demo() {
    let introspect_object = capability::create_object(KernelObjectKind::IntrospectionHandle);
    let audit_object = capability::create_object(KernelObjectKind::AuditQueryHandle);

    klog_info!("AGENT_AUTHORIZED_SPAWN_START");
    thread::spawn_with_capabilities(
        agent_authorized_thread,
        vmm::kernel_pml4_phys(),
        &[(introspect_object, Rights::INTROSPECT), (audit_object, Rights::AUDIT_QUERY)],
    );

    klog_info!("AGENT_STRANGER_SPAWN_START");
    thread::spawn(agent_stranger_thread);

    // Real grant-time policy test: a made-up PortIoRange object,
    // requesting Rights::PORT_IO -- a right no agent may hold under
    // this kernel's Phase 5 policy (see policy.rs::AGENT_MAX_RIGHTS).
    // `spawn_with_capabilities` refuses this specific grant (logging
    // POLICY_GRANT_DENIED + an audit record) while still spawning the
    // process itself, empty-handed -- the process then behaves exactly
    // like the stranger from its own point of view, but the EVIDENCE
    // this generates is different: a policy refusal at spawn time, not
    // just a later resolve-time NoSuchCapability.
    let policy_test_object = capability::create_object(KernelObjectKind::PortIoRange { base: 0, count: 0 });
    klog_info!("AGENT_POLICY_VIOLATOR_SPAWN_START");
    thread::spawn_with_capabilities(
        agent_policy_violator_thread,
        vmm::kernel_pml4_phys(),
        &[(policy_test_object, Rights::PORT_IO)],
    );
}

/// All three entry points below run the IDENTICAL ELF -- the only
/// difference is which capabilities (if any) were granted at spawn time
/// (see `spawn_agent_demo`). Three separate `extern "C" fn`s exist only
/// so their own boot-log lines say which is which for a human reading
/// the evidence; the loaded process itself has no idea which one it is.
extern "C" fn agent_authorized_thread() {
    run_agent_demo("AGENT_AUTHORIZED");
}

extern "C" fn agent_stranger_thread() {
    run_agent_demo("AGENT_STRANGER");
}

extern "C" fn agent_policy_violator_thread() {
    run_agent_demo("AGENT_POLICY_VIOLATOR");
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

        // Real bug found and fixed via Phase 7's shell (see gdt.rs's
        // set_iopb doc comment): agent_demo's own debug output
        // (com1_write_str, its ELF's very first ring-3 action) was
        // never explicitly granted COM1 here -- it only ever worked
        // because the old, buggy GLOBAL TSS IOPB leaked an earlier
        // driver's COM1 grant to every later ring-3 process. Every one
        // of the three demo processes this file spawns needs its own
        // explicit grant now, same as every other driver that writes
        // to COM1 -- deliberately real hardware access (COM1 debug
        // output), not something Rights::INTROSPECT/AUDIT_QUERY alone
        // ever covered.
        let mut com1_table = CapabilityTable::new();
        let com1_cap = driver::create_port_capability(&mut com1_table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&com1_table, com1_cap) {
            Ok(()) => klog_info!("{}_COM1_GRANTED base=0x3f8 count=8", label),
            Err(e) => {
                klog_info!("{}_COM1_GRANT_FAILED {:?}", label, e);
                return;
            }
        }

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
