//! Phase 13 exit criterion 2:
//! "The SDK builds and runs a genuinely new app (not one of the four reference apps)
//! with no kernel or platform-service change required."
//!
//! Demonstrates third-party app development relying solely on `agentic_sdk`.

#![no_std]
#![no_main]

use agentic_sdk::{com1, surface, syscall::syscall1, text_widget::TextRegion};

const INFO_VADDR: u64 = 0x0000_0000_0053_0000;

#[repr(C)]
struct SampleAppInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

const COLS: usize = 40;
const LINES: usize = 8;
const FG: u32 = 0x0000_FF00; // Bright green
const BG: u32 = 0x0010_1010;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const SampleAppInfo);

        com1::write_str("\n[SAMPLE_APP] Genuinely new third-party app built with agentic_sdk\n");
        syscall1(1, 0x5341_4D50); // 'SAMP'

        let mut history_mu = core::mem::MaybeUninit::<TextRegion<LINES, COLS>>::uninit();
        let history_ptr = history_mu.as_mut_ptr();
        TextRegion::init_in_place(history_ptr);

        (*history_ptr).push_line(b"AGENTIC OS THIRD-PARTY SDK APP");
        (*history_ptr).push_line(b"Zero kernel modifications!");
        (*history_ptr).push_line(b"Running cleanly in ring 3.");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);

        surface::present(info.surface_cap);
        syscall1(4, info.ready_token);

        com1::write_str("[SAMPLE_APP] SAMPLE_APP_PASS: third-party SDK app running in ring 3\n");

        let mut count: u32 = 0;
        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                core::hint::spin_loop();
                continue;
            }
            count = count.wrapping_add(1);
            com1::write_str("[SAMPLE_APP] KEY_PRESSED count=");
            com1::write_dec_u64(count as u64);
            com1::write_str("\n");
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
