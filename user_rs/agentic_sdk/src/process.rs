//! Process management APIs for ring-3 processes:
//! SYS_PROCESS_SPAWN (syscall 34)
//! SYS_PROCESS_WAIT (syscall 35)
//! SYS_PROCESS_EXIT (syscall 36)

use crate::syscall::{syscall1, syscall2};

#[repr(C)]
pub struct ProcessSpawnRequest {
    pub path_vaddr: u64,
    pub path_len: u32,
    pub argv_vaddr: u64,
    pub argv_count: u32,
}

/// Spawns a new process by executable path with command line arguments.
/// Returns child `pid` on success, or `u64::MAX` on error.
pub unsafe fn spawn(path: &str, argv: &[&str]) -> u64 {
    let req = ProcessSpawnRequest {
        path_vaddr: path.as_ptr() as u64,
        path_len: path.len() as u32,
        argv_vaddr: argv.as_ptr() as u64,
        argv_count: argv.len() as u32,
    };
    syscall1(34, &req as *const ProcessSpawnRequest as u64)
}

/// Waits for child process `pid` to exit and returns its exit code.
pub unsafe fn waitpid(pid: u64) -> i32 {
    let ret = syscall2(35, pid, 0);
    ret as i32
}

/// Terminates the current process with `exit_code`.
pub unsafe fn exit(code: i32) -> ! {
    syscall1(36, code as u64);
    loop {
        core::hint::spin_loop();
    }
}
