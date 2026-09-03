//! Real proof that Phase 1's long-deferred exit criterion is finally
//! met: "a user-space fault terminates only that process while the
//! system continues" (`docs/ROADMAP.md` §5). Unmet since Phase 1 itself
//! (its own exit-criteria notes named this explicitly), deferred through
//! Phase 2 ("needs the capability/IPC substrate Phase 2 builds" — Phase
//! 2 finished without addressing it either) — closed now, in
//! `idt.rs`/`thread.rs`, once real concurrent ring-3 processes existed
//! to make the gap impossible to ignore any longer.
//!
//! Spawns a real ring-3 process that deliberately executes `hlt` — a
//! CPL0-only instruction, the SAME technique `ring3.rs`'s own original
//! Phase 1 proof used to prove CPL was really 3 (a fault there WAS the
//! proof). The difference now: `idt.rs::recover_or_halt` sees the fault
//! occurred at CS RPL 3 and calls `thread::kill_current_and_reschedule`
//! instead of halting the whole kernel — killing ONLY this process. The
//! actual proof isn't the fault itself (that's been proven since Phase
//! 1); it's that everything ELSE spawned around it — the real driver
//! processes, the kernel-thread demos — keeps running and finishing its
//! own work AFTERWARD, visible directly in the boot log as continued
//! output past this process's own `EXCEPTION`/`PROCESS_KILLED` lines,
//! not silence.

use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const FAULT_DEMO_CODE_VADDR: u64 = 0x0000_0000_0060_0000;
const FAULT_DEMO_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

pub fn spawn_fault_isolation_demo() {
    thread::spawn(fault_demo_thread);
}

extern "C" fn fault_demo_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let code_page = pmm::alloc_page();
        let code_bytes = pmm::p2v_pub(code_page);
        // mov edi, 0xFA17 ("FAULT" marker) ; mov eax, 1 ; syscall (log,
        // proves this process really started) ; hlt (deliberate #GP --
        // proves it, and only it, gets killed)
        let program: [u8; 13] = [
            0xBF, 0x17, 0xFA, 0x00, 0x00, // mov edi, 0xFA17
            0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1
            0x0F, 0x05, // syscall
            0xF4, // hlt
        ];
        core::ptr::copy_nonoverlapping(program.as_ptr(), code_bytes, program.len());
        vmm::map_page_in(space, FAULT_DEMO_CODE_VADDR, code_page, vmm::PAGE_USER);
        // deliberately no PAGE_NO_EXECUTE -- this page must be executable

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            FAULT_DEMO_STACK_VADDR,
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
            "FAULT_ISOLATION_DEMO_ENTER entry=0x{:x} stack=0x{:x} (about to deliberately fault)",
            FAULT_DEMO_CODE_VADDR,
            FAULT_DEMO_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(FAULT_DEMO_CODE_VADDR, FAULT_DEMO_STACK_VADDR + 4096);
    }
}
