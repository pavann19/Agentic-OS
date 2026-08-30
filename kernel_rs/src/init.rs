//! `init` — the first real user-space process, first half of Phase 3's
//! "init and a service manager as the first user-space processes"
//! (`docs/ROADMAP.md`). Unlike the Phase 1/2 `demo_ring3` proof (which
//! stays feature-gated, a deliberate proof-of-mechanism), this runs
//! unconditionally as part of normal boot — it IS the boot path's first
//! user-space process now, not a demo of one.
//!
//! Same hand-built-machine-code technique as `demo_ring3_thread`
//! (`main.rs`) for the same reason stated there: this kernel has no ELF
//! loader for arbitrary user binaries yet (that's real work the next
//! Phase 3 item, "first user-space drivers", needs and will build). What
//! makes this a real `init` rather than another demo is what the code
//! DOES: issues syscall 3 (SVC_START), a real capability-gated IPC send
//! that only succeeds if `service_manager.rs`'s thread is alive and
//! waiting to receive it — proving a genuine cross-process handoff, not
//! just another isolated ring-3 execution proof.
//!
//! Same CR3-switch-must-happen-from-a-spawned-thread constraint
//! `demo_ring3_thread` documents applies here identically — this function
//! is meant to run as its own spawned thread, never inlined into
//! `kernel_main`.

use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const INIT_CODE_VADDR: u64 = 0x0000_0000_0060_0000;
const INIT_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

/// Spawns `service_manager_thread` (so it's alive and blocked on
/// `ipc::receive` before init's syscall can reach it — real ordering
/// enforced by IPC rendezvous, not a race hoped away by scheduling luck:
/// if init's syscall 3 ran before the service manager called
/// `ipc::receive`, the message would sit `MESSAGE_PENDING` in the
/// endpoint until the service manager thread next runs and picks it up —
/// correct either way, since `ipc.rs`'s rendezvous is a real mailbox, not
/// a same-instant handoff), sets up the real capability init's syscall 3
/// will use, then spawns `init_thread` itself.
pub fn spawn_init() {
    syscall::init_service_ipc();
    thread::spawn(crate::service_manager::service_manager_thread);
    thread::spawn(init_thread);
}

extern "C" fn init_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let code_page = pmm::alloc_page();
        let code_bytes = pmm::p2v_pub(code_page);
        // mov edi, 0xC0DE ; mov eax, 3 ; syscall (SVC_START) ; hlt
        let program: [u8; 13] = [
            0xBF, 0xDE, 0xC0, 0x00, 0x00, // mov edi, 0xC0DE
            0xB8, 0x03, 0x00, 0x00, 0x00, // mov eax, 3
            0x0F, 0x05, // syscall
            0xF4, // hlt
        ];
        core::ptr::copy_nonoverlapping(program.as_ptr(), code_bytes, program.len());
        vmm::map_page_in(space, INIT_CODE_VADDR, code_page, vmm::PAGE_USER);
        // deliberately no PAGE_NO_EXECUTE -- this page must be executable

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            INIT_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!(
            "INIT_ENTER entry=0x{:x} stack=0x{:x}",
            INIT_CODE_VADDR,
            INIT_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(INIT_CODE_VADDR, INIT_STACK_VADDR + 4096);
    }
}
