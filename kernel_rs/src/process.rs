//! Milestone 2 — Generic on-demand process execution substrate.
//! Provides `SYS_PROCESS_SPAWN` (34), `SYS_PROCESS_WAIT` (35), and `SYS_PROCESS_EXIT` (36).
//!
//! Spawn hardening (docs/TASK_SPAWN_HARDENING.md) — two real gaps closed:
//!
//! 1. **Real path resolution.** The previous implementation matched the `path`
//!    argument against a few hardcoded string patterns and always loaded one
//!    fixed `include_bytes!`-embedded binary regardless of what path was
//!    requested. This pass wires the existing `file_service` FS_OP_LOOKUP +
//!    file-read IPC path (`virtio_blk_driver`'s own `driver_resolve_path`
//!    already uses the same `ext2::resolve_path` underneath) to find the
//!    target inode on the REAL filesystem and load the REAL file's bytes —
//!    not any embedded binary.  `child_proc` is kept as a genuine test
//!    fixture: kernel-side setup in `net_client_app.rs` writes its ELF to
//!    disk at `/bin/child` before ring 3 ever runs, and `net_client` spawns
//!    it BY PATH to prove the whole chain.
//!
//! 2. **Real capability gate (`Rights::EXEC`).** Every other syscall class in
//!    this kernel already gates on a purpose-specific `Rights` bit via
//!    `thread::resolve_current_capability`. `sys_spawn` was the single gap.
//!    Fixed the same way: `Rights::EXEC` (bit 14, `capability.rs`) is checked
//!    as the FIRST thing `sys_spawn` does — deny + `AuditEvent::Denied` +
//!    early return on failure, exactly the same pattern as syscalls 7/9/32/33
//!    (`INTROSPECT`/`AUDIT_QUERY`/`INTROSPECT_WINDOWS`/`AGENT_UI_ACTION`).
//!    Only a process explicitly granted an `ExecHandle` capability with
//!    `Rights::EXEC` at spawn time may call this syscall at all.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::{audit, capability, klog_info, pmm, thread, vmm};

/// Slot index the `ExecHandle` capability is granted into by
/// `net_client_app.rs`. By convention every new per-process capability kind
/// in this kernel is granted at a fixed, predictable table index so the
/// ring-3 process knows which slot to pass. Slot 3 for net_client (after
/// Surface=0, Socket=1, PortIo=2). Unauthorized processes receive nothing at
/// this slot, so `resolve_current_capability(EXEC_CAP_SLOT, EXEC)` fails.
pub const EXEC_CAP_SLOT: capability::CapId = 3;

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

/// Maximum ELF binary size we will load on-demand.
const MAX_ELF_BYTES: usize = 512 * 1024;

/// Sends an FS_OP_LOOKUP request through `file_service` and spin-polls for
/// the reply, returning the resolved inode or 0 on failure.  Called from
/// `sys_spawn` which runs with interrupts enabled so the virtio-blk driver
/// thread can run and service the request while we spin.
fn lookup_path_inode(path: &str) -> u32 {
    const FS_OP_LOOKUP: u8 = 2;
    let arg0 = (FS_OP_LOOKUP as u32) << 24;
    // Pass path bytes as a kernel-address pointer; file_service::write_file
    // takes a pml4 + vaddr pair — we use the kernel pml4 here since this is
    // kernel code and the path buffer is a kernel stack slice.
    let req_id = crate::file_service::write_file(
        vmm::kernel_pml4_phys(),
        arg0,
        path.as_ptr() as u64,
        path.len() as u32,
        0,
    );
    if req_id == 0 {
        klog_info!("SYS_PROCESS_SPAWN_LOOKUP_NO_SERVER path=\"{}\"", path);
        return 0;
    }
    let mut out = [0u8; 16];
    let out_vaddr = out.as_mut_ptr() as u64;
    for _ in 0..10_000_000u32 {
        let n = crate::file_service::poll_reply(
            vmm::kernel_pml4_phys(),
            req_id,
            out_vaddr,
            16,
        );
        if n != u64::MAX {
            if n >= 4 {
                return u32::from_le_bytes(out[0..4].try_into().unwrap_or([0u8; 4]));
            }
            return 0;
        }
        unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    }
    klog_info!("SYS_PROCESS_SPAWN_LOOKUP_TIMEOUT path=\"{}\"", path);
    0
}

/// Reads the file at `inode` via `file_service` and spin-polls until done,
/// returning actual byte count on success (0 on error/timeout).
fn read_file_bytes(inode: u32, buf: &mut [u8]) -> usize {
    let req_id = crate::file_service::request_file_internal(inode);
    if req_id == 0 {
        klog_info!("SYS_PROCESS_SPAWN_READ_NO_SERVER inode={}", inode);
        return 0;
    }
    let out_vaddr = buf.as_mut_ptr() as u64;
    let max_len = buf.len() as u32;
    for _ in 0..10_000_000u32 {
        let n = crate::file_service::poll_reply(
            vmm::kernel_pml4_phys(),
            req_id,
            out_vaddr,
            max_len,
        );
        if n != u64::MAX {
            return (n as usize).min(buf.len());
        }
        unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    }
    klog_info!("SYS_PROCESS_SPAWN_READ_TIMEOUT inode={}", inode);
    0
}

/// Dispatches SYS_PROCESS_SPAWN (syscall 34).
///
/// Hardening fixes (docs/TASK_SPAWN_HARDENING.md):
///
/// 1. **Capability gate first**: resolves `Rights::EXEC` against the calling
///    thread's OWN cap_table via `thread::resolve_current_capability`. On
///    failure: log + `AuditEvent::Denied` + return `u64::MAX` immediately,
///    before any path parsing, allocation, or IPC. Matches the pattern
///    syscalls 7/9/32/33 already use for their own per-process rights.
///
/// 2. **Real path resolution**: sends `FS_OP_LOOKUP` to the file-service
///    driver via `file_service::write_file` (the same IPC path the SDK's own
///    `file_service::lookup` uses from user space), polls for the inode, then
///    reads the ELF bytes via `file_service::request_file` + `poll_reply`.
///    Any ELF on the real ext2 filesystem can be spawned by path; no
///    hardcoded pattern match, no embedded binary.
pub fn sys_spawn(req_vaddr: u64) -> u64 {
    // ── 1. Capability gate ───────────────────────────────────────────────────
    // Resolved FIRST — before any user pointer is touched — so an
    // unauthorized caller sees a fast, audited denial with no side effects.
    // `CapabilityTable::resolve` already emits `AuditEvent::Denied` on its
    // own (see capability.rs doc comment); we additionally log a kernel
    // message to make the denial visible in the serial transcript.
    if let Err(_) = thread::resolve_current_capability(EXEC_CAP_SLOT, capability::Rights::EXEC) {
        klog_info!("SYS_PROCESS_SPAWN_EXEC_DENIED: caller lacks ExecHandle/Rights::EXEC");
        audit::record(audit::AuditEvent::Denied { cap_id: EXEC_CAP_SLOT });
        return u64::MAX;
    }

    // ── 2. Read ProcessSpawnRequest from user memory ─────────────────────────
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

    // ── 3. Real path resolution via file_service IPC ─────────────────────────
    // FS_OP_LOOKUP → inode (via virtio_blk_driver's ext2::resolve_path), then
    // request_file(inode) → ELF bytes. This is the same IPC chain the SDK's
    // own `file_service::lookup` + `read_file_path` uses from ring-3; calling
    // it from kernel context is valid because the virtio_blk_driver thread is
    // already scheduled and will service the IPC while we spin below.
    let inode = lookup_path_inode(path_str);
    if inode == 0 {
        klog_info!("SYS_PROCESS_SPAWN_PATH_NOT_FOUND: \"{}\"", path_str);
        return u64::MAX;
    }
    klog_info!("SYS_PROCESS_SPAWN_RESOLVED path=\"{}\" inode={}", path_str, inode);

    let mut elf_buf = alloc::vec![0u8; MAX_ELF_BYTES];
    let elf_len = read_file_bytes(inode, &mut elf_buf);
    if elf_len == 0 {
        klog_info!("SYS_PROCESS_SPAWN_ELF_READ_FAILED inode={}", inode);
        return u64::MAX;
    }
    klog_info!("SYS_PROCESS_SPAWN_ELF_LOADED inode={} bytes={}", inode, elf_len);

    // ── 4. Load ELF into a new isolated address space ────────────────────────
    let space = unsafe { vmm::new_address_space() };
    let entry = match unsafe { crate::elf::load(space, &elf_buf[..elf_len]) } {
        Ok(e) => e,
        Err(e) => {
            klog_info!("SYS_PROCESS_SPAWN_ELF_LOAD_FAILED {:?}", e);
            unsafe { vmm::destroy_address_space(space) };
            return u64::MAX;
        }
    };
    drop(elf_buf);

    // User stack: 4 pages (16 KB) at 0x00B0_0000
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


