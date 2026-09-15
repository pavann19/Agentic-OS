//! Minimal child process ELF binary spawned on-demand in Milestone 2.
//! Demonstrates running a second ELF binary by path from a running ring-3 process,
//! writing output to serial COM1, and returning a real exit code to waitpid.

#![no_std]
#![no_main]

use agentic_sdk::{com1, process};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1::write_str("\n[CHILD_PROC] spawned on-demand in ring 3 via generic exec syscall\n");
    com1::write_str("[CHILD_PROC] executing child task and exiting with code 42\n");
    unsafe {
        process::exit(42);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
