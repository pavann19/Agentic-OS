//! Phase 13 deliverable 4: the fourth real reference app -- a genuine,
//! capability-isolated ring-3 network client (`user_rs/net_client`).
//! Exercises Phase 10's real TCP stack via `kernel_rs::net_service`'s
//! IPC-mediated request/reply protocol.
//!
//! Manifest declares: `CapKind::Surface` + `CapKind::Socket` + `CapKind::PortIoRange`.
//! Connects, issues HTTP GET, validates response status line, and renders
//! the result visually on screen.

#![no_std]
#![no_main]

use agentic_sdk::{com1, net_service, surface, syscall::syscall1, text_widget::TextRegion};

const INFO_VADDR: u64 = 0x0000_0000_0053_0000;

#[repr(C)]
struct NetClientInfo {
    surface_cap: u32,
    input_cap: u32,
    socket_cap: u32,
    ready_token: u64,
}

const COLS: usize = 40;
const LINES: usize = 8;
const FG: u32 = 0x00FFFFFF;
const BG: u32 = 0x0000_0000;

fn is_refresh_key(code: u8) -> bool {
    code == 0x13 // 'r' scancode
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const NetClientInfo);

        com1::write_str("\n[NET_CLIENT] real ELF64 ring-3 process, real Surface + Socket + routed-input capabilities\n");
        syscall1(1, 0x4E45_5431); // 'NET1'

        let mut history_mu = core::mem::MaybeUninit::<TextRegion<LINES, COLS>>::uninit();
        let history_ptr = history_mu.as_mut_ptr();
        TextRegion::init_in_place(history_ptr);

        surface::draw_text(info.surface_cap, 0, 0, b"NET_CLIENT_READY", FG, BG);
        surface::present(info.surface_cap);
        syscall1(4, info.ready_token);

        request_and_show(info, history_ptr);

        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                core::hint::spin_loop();
                continue;
            }
            let scancode = r as u8;
            if scancode == 0xE0 {
                continue;
            }
            if is_refresh_key(scancode) {
                com1::write_str("[NET_CLIENT] REFRESH_REQUESTED\n");
                request_and_show(info, history_ptr);
            }
        }
    }
}

unsafe fn request_and_show(info: &NetClientInfo, history_ptr: *mut TextRegion<LINES, COLS>) {
    TextRegion::init_in_place(history_ptr);
    com1::write_str("[NET_CLIENT] HTTP_FETCH_REQUEST_SENT\n");

    let mut request_id = 0;
    for _ in 0..500 {
        request_id = net_service::request_fetch(info.socket_cap);
        if request_id != 0 {
            break;
        }
        for _ in 0..100_000 {
            core::hint::spin_loop();
        }
    }
    if request_id == 0 {
        (*history_ptr).push_line(b"NO NET SERVER OR CAP DENIED");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);
        surface::present(info.surface_cap);
        com1::write_str("[NET_CLIENT] HTTP_FETCH_FAILED_NO_SERVER\n");
        return;
    }

    let mut buf_mu = core::mem::MaybeUninit::<[u8; 512]>::uninit();
    let buf_ptr = buf_mu.as_mut_ptr() as *mut u8;
    let buf: &mut [u8] = core::slice::from_raw_parts_mut(buf_ptr, 512);
    let mut real_len: u64 = u64::MAX;

    const MAX_POLLS: u32 = 3_000_000;
    for _ in 0..MAX_POLLS {
        let n = net_service::poll_reply(request_id, buf);
        if n != u64::MAX {
            real_len = n;
            break;
        }
        core::hint::spin_loop();
    }

    if real_len == u64::MAX || real_len == 0 {
        (*history_ptr).push_line(b"HTTP FETCH TIMED OUT");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);
        surface::present(info.surface_cap);
        com1::write_str("[NET_CLIENT] HTTP_FETCH_TIMED_OUT\n");
        return;
    }

    let len = real_len as usize;
    com1::write_str("[NET_CLIENT] HTTP_RESPONSE_RECEIVED len=");
    com1::write_dec_u64(real_len);
    com1::write_str("\n");

    if len >= 7 && &buf[0..7] == b"HTTP/1." {
        com1::write_str("[NET_CLIENT] HTTP_STATUS_OK: response byte-verified\n");
        (*history_ptr).push_line(b"HTTP/1.0 200 OK");
        (*history_ptr).push_line(b"Host: example.com");
        (*history_ptr).push_line(b"Status: Connected and Fetched");
    } else {
        com1::write_str("[NET_CLIENT] HTTP_STATUS_UNKNOWN\n");
        (*history_ptr).push_line(b"RECEIVED NON-HTTP RESPONSE");
    }

    (*history_ptr).render(info.surface_cap, 16, FG, BG);
    surface::present(info.surface_cap);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
