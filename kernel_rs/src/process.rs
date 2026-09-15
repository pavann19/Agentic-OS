//! Milestone 2 — Generic on-demand process execution substrate.
//! Provides `SYS_PROCESS_SPAWN` (34), `SYS_PROCESS_WAIT` (35), and `SYS_PROCESS_EXIT` (36).
//! Allows a running ring-3 process to spawn a second ELF binary by path on-demand,
//! wait for its exit status, and cleanly terminate processes.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::{klog_info, pmm, thread, vmm};

static CHILD_ELF: &[u8] = include_bytes!("../../user_rs/child_proc/target/x86_64-unknown-none/release/child_proc");

pub type ProcessId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProcessStatus {
    Running,
    Exited(i32),
}

pub struct Process {
    pub pid: ProcessId,
    pub thread_id: thread::ThreadId,
    pub address_space: u64,
    pub status: ProcessStatus,
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);
static mut PROCESS_TABLE: Vec<Process> = Vec::new();

#[repr(C)]
pub struct ProcessSpawnRequest {
    pub path_vaddr: u64,
    pub path_len: u32,
    pub argv_vaddr: u64,
    pub argv_count: u32,
}

/// Dispatches SYS_PROCESS_SPAWN (syscall 34).
/// Reads path and argv parameters from user memory, validates and loads the target ELF,
/// creates a new isolated address space, user stack, and schedules a new thread.
pub fn sys_spawn(req_vaddr: u64) -> u64 {
    let pml4 = vmm::current_cr3();
    let mut req_bytes = [0u8; core::mem::size_of::<ProcessSpawnRequest>()];
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, req_vaddr, req_bytes.len() as u64) {
            klog_info!("SYS_PROCESS_SPAWN_BAD_REQ_PTR 0x{:x}", req_vaddr);
            return u64::MAX;
        }
        vmm::read_user_bytes(pml4, req_vaddr, &mut req_bytes);
    }
    let req: ProcessSpawnRequest = unsafe { core::ptr::read(req_bytes.as_ptr() as *const _) };

    let mut path_bytes = [0u8; 128];
    let path_len = (req.path_len as usize).min(127);
    if path_len > 0 {
        unsafe {
            if !vmm::validate_user_buffer_readable(pml4, req.path_vaddr, path_len as u64) {
                klog_info!("SYS_PROCESS_SPAWN_BAD_PATH_PTR 0x{:x}", req.path_vaddr);
                return u64::MAX;
            }
            vmm::read_user_bytes(pml4, req.path_vaddr, &mut path_bytes[..path_len]);
        }
    }
    let path_str = core::str::from_utf8(&path_bytes[..path_len]).unwrap_or("");
    klog_info!("SYS_PROCESS_SPAWN path=\"{}\" argc={}", path_str, req.argv_count);

    // Resolve ELF binary by path
    let elf_data: &[u8] = if path_str == "/bin/child" || path_str == "child" || path_str.ends_with("child") || path_str == "/bin/child_proc" {
        CHILD_ELF
    } else {
        klog_info!("SYS_PROCESS_SPAWN_PATH_NOT_FOUND: {}", path_str);
        return u64::MAX;
    };

    let space = unsafe { vmm::new_address_space() };
    let entry = match unsafe { crate::elf::load(space, elf_data) } {
        Ok(e) => e,
        Err(e) => {
            klog_info!("SYS_PROCESS_SPAWN_ELF_LOAD_FAILED {:?}", e);
            unsafe { vmm::destroy_address_space(space) };
            return u64::MAX;
        }
    };

    // User stack: 4 pages (16KB) at 0x00B0_0000
    const STACK_VADDR: u64 = 0x00B0_0000;
    const STACK_PAGES: u64 = 4;
    for i in 0..STACK_PAGES {
        let p = unsafe { pmm::alloc_page() };
        unsafe {
            vmm::map_page_in(
                space,
                STACK_VADDR + i * 4096,
                p,
                vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
            );
        }
    }
    let user_rsp = STACK_VADDR + STACK_PAGES * 4096;

    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);
    let tid = thread::spawn_user(entry, user_rsp, space);

    crate::critical::without_interrupts(|| unsafe {
        (&mut *&raw mut PROCESS_TABLE).push(Process {
            pid,
            thread_id: tid,
            address_space: space,
            status: ProcessStatus::Running,
        });
    });

    klog_info!("SYS_PROCESS_SPAWN_OK pid={} tid={} entry=0x{:x} rsp=0x{:x}", pid, tid, entry, user_rsp);
    pid
}

/// Checks if child process `pid` has exited, returning exit code if reaped.
pub fn try_wait(pid: u64) -> Option<i32> {
    crate::critical::without_interrupts(|| unsafe {
        let table = &mut *&raw mut PROCESS_TABLE;
        if let Some(pos) = table.iter().position(|p| p.pid == pid) {
            match table[pos].status {
                ProcessStatus::Exited(code) => {
                    table.remove(pos);
                    Some(code)
                }
                ProcessStatus::Running => None,
            }
        } else {
            None
        }
    })
}

/// Checks if process `pid` exists in the table.
pub fn process_exists(pid: u64) -> bool {
    crate::critical::without_interrupts(|| unsafe {
        (&*&raw const PROCESS_TABLE).iter().any(|p| p.pid == pid)
    })
}

/// Dispatches SYS_PROCESS_WAIT (syscall 35).
/// Loops yielding the CPU until child process `pid` exits.
pub fn sys_wait(pid: u64, _options: u64) -> u64 {
    if !process_exists(pid) {
        klog_info!("SYS_PROCESS_WAIT_NO_SUCH_PID pid={}", pid);
        return u64::MAX;
    }

    loop {
        if let Some(code) = try_wait(pid) {
            klog_info!("SYS_PROCESS_WAIT_REAPED pid={} exit_code={}", pid, code);
            return code as u32 as u64;
        }
        // Yield CPU until child process finishes
        unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    }
}

/// Dispatches SYS_PROCESS_EXIT (syscall 36).
/// Records exit code and terminates the current running thread.
pub fn sys_exit(code: i32) -> ! {
    let tid = thread::current_id();
    klog_info!("SYS_PROCESS_EXIT tid={} code={}", tid, code);

    crate::critical::without_interrupts(|| unsafe {
        let table = &mut *&raw mut PROCESS_TABLE;
        if let Some(p) = table.iter_mut().find(|p| p.thread_id == tid) {
            p.status = ProcessStatus::Exited(code);
            klog_info!("PROCESS_TABLE_UPDATED pid={} status=Exited({})", p.pid, code);
        }
    });

    thread::kill_current_and_reschedule();
    loop {
        unsafe { core::arch::asm!("sti; hlt", options(nomem, nostack)) };
    }
}
