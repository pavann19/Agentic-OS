//! Distinct second test binary for spawn hardening verification (docs/TASK_SPAWN_HARDENING.md).
//! Demonstrates that path resolution is genuinely generic and not pattern-matched.

#![no_std]
#![no_main]

use agentic_sdk::{com1, process};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1::write_str("\n[HELPER_PROC] helper binary spawned by distinct path in ring 3\n");
    com1::write_str("[HELPER_PROC] executing helper task and exiting with code 84\n");
    unsafe {
        process::exit(84);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
