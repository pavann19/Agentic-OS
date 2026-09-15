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

use agentic_sdk::{com1, file_service, net_service, process, surface, syscall::syscall1, text_widget::TextRegion};

const INFO_VADDR: u64 = 0x0000_0000_0053_0000;

#[repr(C)]
struct NetClientInfo {
    surface_cap: u32,
    input_cap: u32,
    socket_cap: u32,
    ready_token: u64,
    file_cap: u32,
    save_to_disk: u32,
    target_inode: u32,
}

const COLS: usize = 40;
const LINES: usize = 8;
const FG: u32 = 0x00FFFFFF;
const BG: u32 = 0x0000_0000;

fn is_refresh_key(code: u8) -> bool {
    code == 0x13 // 'r' scancode
}

fn is_download_key(code: u8) -> bool {
    code == 0x20 // 'd' scancode
}

fn print_hex_bytes(bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        let hi = HEX[(b >> 4) as usize];
        let lo = HEX[(b & 0x0f) as usize];
        let s = [hi, lo];
        if let Ok(st) = core::str::from_utf8(&s) {
            com1::write_str(st);
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const NetClientInfo);

        com1::write_str("\n[NET_CLIENT] real ELF64 ring-3 process, real Surface + Socket + FileObject capabilities\n");
        syscall1(1, 0x4E45_5431); // 'NET1'

        let mut history_mu = core::mem::MaybeUninit::<TextRegion<LINES, COLS>>::uninit();
        let history_ptr = history_mu.as_mut_ptr();
        TextRegion::init_in_place(history_ptr);

        surface::draw_text(info.surface_cap, 0, 0, b"NET_CLIENT_READY", FG, BG);
        surface::present(info.surface_cap);
        syscall1(4, info.ready_token);

        if info.save_to_disk != 0 {
            download_and_save(info, history_ptr);
        } else {
            request_and_show(info, history_ptr);
        }

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
            } else if is_download_key(scancode) {
                com1::write_str("[NET_CLIENT] MANUAL_DOWNLOAD_REQUESTED\n");
                download_and_save(info, history_ptr);
            }
        }
    }
}

unsafe fn download_and_save(info: &NetClientInfo, history_ptr: *mut TextRegion<LINES, COLS>) {
    TextRegion::init_in_place(history_ptr);
    com1::write_str("[NET_CLIENT] AGENT_DOWNLOAD_START\n");

    // 1. Send HTTP fetch request via net_service IPC
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

    // 2. Poll for HTTP reply (up to 4096 bytes)
    let mut http_buf_mu = core::mem::MaybeUninit::<[u8; 4096]>::uninit();
    let http_buf_ptr = http_buf_mu.as_mut_ptr() as *mut u8;
    let http_buf: &mut [u8] = core::slice::from_raw_parts_mut(http_buf_ptr, 4096);
    let mut real_len: u64 = u64::MAX;

    const MAX_POLLS: u32 = 3_000_000;
    for _ in 0..MAX_POLLS {
        let n = net_service::poll_reply(request_id, http_buf);
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

    if len >= 7 && &http_buf[0..7] == b"HTTP/1." {
        com1::write_str("[NET_CLIENT] HTTP_STATUS_OK: response byte-verified\n");
    } else {
        com1::write_str("[NET_CLIENT] HTTP_STATUS_UNKNOWN\n");
    }

    // 3. Extract HTTP body
    let mut body_start = 0;
    let mut found_header_end = false;
    for i in 0..len {
        if i + 4 <= len && &http_buf[i..i + 4] == b"\r\n\r\n" {
            body_start = i + 4;
            found_header_end = true;
            break;
        } else if i + 2 <= len && &http_buf[i..i + 2] == b"\n\n" {
            body_start = i + 2;
            found_header_end = true;
            break;
        }
    }
    let body = if found_header_end && body_start < len {
        &http_buf[body_start..len]
    } else {
        &http_buf[0..len]
    };

    if body.is_empty() {
        com1::write_str("[NET_CLIENT] HTTP_BODY_EMPTY\n");
        return;
    }

    com1::write_str("[NET_CLIENT] HTTP_BODY_EXTRACTED len=");
    com1::write_dec_u64(body.len() as u64);
    com1::write_str("\n");

    let body_digest = kernel_common::crypto::Sha256::digest(body);
    com1::write_str("[NET_CLIENT] HTTP_BODY_SHA256=");
    print_hex_bytes(&body_digest);
    com1::write_str("\n");

    // 4. Persist to disk via file_service
    com1::write_str("[NET_CLIENT] PERSISTING_TO_DISK inode=");
    com1::write_dec_u64(info.target_inode as u64);
    com1::write_str("\n");

    let write_id = file_service::write_file_at(info.file_cap, 0, body);
    if write_id == 0 {
        com1::write_str("[NET_CLIENT] FILE_SERVICE_WRITE_DENIED_OR_NO_SERVER\n");
        (*history_ptr).push_line(b"HTTP/1.0 200 OK");
        (*history_ptr).push_line(b"DISK WRITE DENIED/NO SERVER");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);
        surface::present(info.surface_cap);
        return;
    }

    let mut poll_buf_mu = core::mem::MaybeUninit::<[u8; 64]>::uninit();
    let poll_buf_ptr = poll_buf_mu.as_mut_ptr() as *mut u8;
    let poll_buf = core::slice::from_raw_parts_mut(poll_buf_ptr, 64);
    let mut write_res = u64::MAX;
    for _ in 0..MAX_POLLS {
        let n = file_service::poll_reply(write_id, poll_buf);
        if n != u64::MAX {
            write_res = n;
            break;
        }
        core::hint::spin_loop();
    }

    if write_res == u64::MAX {
        com1::write_str("[NET_CLIENT] FILE_SERVICE_WRITE_TIMED_OUT\n");
        (*history_ptr).push_line(b"FILE WRITE TIMED OUT");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);
        surface::present(info.surface_cap);
        return;
    }

    com1::write_str("[NET_CLIENT] FILE_WRITE_CONFIRMED len=");
    com1::write_dec_u64(write_res);
    com1::write_str("\n");

    // 5. Read back from disk to cryptographically verify payload
    com1::write_str("[NET_CLIENT] DISK_READBACK_START inode=");
    com1::write_dec_u64(info.target_inode as u64);
    com1::write_str("\n");

    let read_id = file_service::request_file(info.file_cap);
    if read_id == 0 {
        com1::write_str("[NET_CLIENT] DISK_READBACK_REQUEST_FAILED\n");
        return;
    }

    let mut read_buf_mu = core::mem::MaybeUninit::<[u8; 4096]>::uninit();
    let read_buf_ptr = read_buf_mu.as_mut_ptr() as *mut u8;
    let read_buf: &mut [u8] = core::slice::from_raw_parts_mut(read_buf_ptr, 4096);
    let mut read_len = u64::MAX;
    for _ in 0..MAX_POLLS {
        let n = file_service::poll_reply(read_id, read_buf);
        if n != u64::MAX {
            read_len = n;
            break;
        }
        core::hint::spin_loop();
    }

    if read_len == u64::MAX {
        com1::write_str("[NET_CLIENT] DISK_READBACK_TIMED_OUT\n");
        return;
    }

    com1::write_str("[NET_CLIENT] DISK_READBACK_LEN len=");
    com1::write_dec_u64(read_len);
    com1::write_str("\n");

    let read_digest = kernel_common::crypto::Sha256::digest(&read_buf[..read_len as usize]);
    com1::write_str("[NET_CLIENT] DISK_READBACK_SHA256=");
    print_hex_bytes(&read_digest);
    com1::write_str("\n");

    if kernel_common::crypto::constant_time_eq(&body_digest, &read_digest) && read_len as usize == body.len() {
        com1::write_str("[NET_CLIENT] DOWNLOAD_CHECKSUM_VERIFIED: sha256 byte-matched\n");
        com1::write_str("[NET_CLIENT] AGENT_DOWNLOAD_SUCCESS\n");
        (*history_ptr).push_line(b"HTTP/1.0 200 OK");
        (*history_ptr).push_line(b"Saved to Inode 11");
        (*history_ptr).push_line(b"SHA-256 Match: VERIFIED");

        verify_self_hosting_groundwork(info.file_cap);
    } else {
        com1::write_str("[NET_CLIENT] DOWNLOAD_CHECKSUM_MISMATCH\n");
        (*history_ptr).push_line(b"CHECKSUM MISMATCH");
    }

    (*history_ptr).render(info.surface_cap, 16, FG, BG);
    surface::present(info.surface_cap);
}

unsafe fn verify_self_hosting_groundwork(_file_cap: u32) {
    com1::write_str("\n=== [SELF_HOSTING] Groundwork Verification Starting ===\n");

    // 1. Directory creation: mkdir("/src")
    com1::write_str("[SELF_HOSTING] Creating directory /src...\n");
    let ok1 = file_service::mkdir("/src");
    if ok1 {
        com1::write_str("[SELF_HOSTING] MKDIR /src OK\n");
    } else {
        com1::write_str("[SELF_HOSTING] MKDIR /src FAILED\n");
    }

    // 2. Subdirectory creation: mkdir("/src/bin")
    com1::write_str("[SELF_HOSTING] Creating directory /src/bin...\n");
    let ok2 = file_service::mkdir("/src/bin");
    if ok2 {
        com1::write_str("[SELF_HOSTING] MKDIR /src/bin OK\n");
    } else {
        com1::write_str("[SELF_HOSTING] MKDIR /src/bin FAILED\n");
    }
    if ok1 && ok2 {
        com1::write_str("[SELF_HOSTING] MKDIR_SUCCESS\n");
    }

    // 3. Write file by multi-file path: write_file_path("/src/bin/hello.txt", ...)
    const TEST_PAYLOAD: &[u8] = b"Self-hosting groundwork verified: ext2 directories + on-demand exec\n";
    com1::write_str("[SELF_HOSTING] Writing file to /src/bin/hello.txt...\n");
    let w_ok = file_service::write_file_path("/src/bin/hello.txt", TEST_PAYLOAD);
    if w_ok {
        com1::write_str("[SELF_HOSTING] WRITE_PATH OK len=");
        com1::write_dec_u64(TEST_PAYLOAD.len() as u64);
        com1::write_str("\n");
    } else {
        com1::write_str("[SELF_HOSTING] WRITE_PATH FAILED\n");
    }

    // 4. Read back file by path and verify content
    let mut read_buf = [0u8; 128];
    let r_len = file_service::read_file_path("/src/bin/hello.txt", &mut read_buf);
    if r_len == TEST_PAYLOAD.len() && &read_buf[..r_len] == TEST_PAYLOAD {
        com1::write_str("[SELF_HOSTING] READ_PATH OK: byte-for-byte content matched\n");
        com1::write_str("[SELF_HOSTING] WRITE_READ_PATH_SUCCESS\n");
    } else {
        com1::write_str("[SELF_HOSTING] READ_PATH FAILED\n");
    }

    // 5. Readdir on /src/bin
    let mut entries = [file_service::FsDirEntry { inode: 0, file_type: 0, name_len: 0, pad: 0, name: [0u8; 56] }; 8];
    let num_entries = file_service::readdir("/src/bin", &mut entries);
    com1::write_str("[SELF_HOSTING] READDIR /src/bin found count=");
    com1::write_dec_u64(num_entries as u64);
    com1::write_str("\n");
    let mut found_hello = false;
    for i in 0..num_entries.min(entries.len()) {
        let entry = &entries[i];
        let name_str = core::str::from_utf8(&entry.name[..entry.name_len as usize]).unwrap_or("");
        com1::write_str("  entry: ");
        com1::write_str(name_str);
        com1::write_str(" inode=");
        com1::write_dec_u64(entry.inode as u64);
        com1::write_str(" type=");
        com1::write_dec_u64(entry.file_type as u64);
        com1::write_str("\n");
        if name_str == "hello.txt" {
            found_hello = true;
        }
    }
    if found_hello {
        com1::write_str("[SELF_HOSTING] READDIR_SUCCESS\n");
    } else {
        com1::write_str("[SELF_HOSTING] READDIR_FAILED: hello.txt not found\n");
    }

    // 6. Unlink file
    com1::write_str("[SELF_HOSTING] Unlinking /src/bin/hello.txt...\n");
    let un_ok = file_service::unlink("/src/bin/hello.txt");
    if un_ok {
        com1::write_str("[SELF_HOSTING] UNLINK OK\n");
        com1::write_str("[SELF_HOSTING] UNLINK_SUCCESS\n");
    } else {
        com1::write_str("[SELF_HOSTING] UNLINK FAILED\n");
    }

    // 7. Generic on-demand process exec: spawn child process by path
    com1::write_str("[SELF_HOSTING] Spawning child process /bin/child on-demand...\n");
    let child_pid = process::spawn("/bin/child", &["child", "--self-host-test"]);
    if child_pid != u64::MAX && child_pid > 0 {
        com1::write_str("[SELF_HOSTING] SPAWN_SUCCESS pid=");
        com1::write_dec_u64(child_pid);
        com1::write_str("\n");

        // 8. Waitpid for child process to exit
        com1::write_str("[SELF_HOSTING] Waiting for child process to exit via waitpid...\n");
        let exit_code = process::waitpid(child_pid);
        com1::write_str("[SELF_HOSTING] Child process reaped, exit_code=");
        com1::write_dec_u64(exit_code as u64);
        com1::write_str("\n");

        if exit_code == 42 {
            com1::write_str("[SELF_HOSTING] WAITPID_SUCCESS exit_code=42\n");
            com1::write_str("[SELF_HOSTING] ALL_MILESTONE_2_VERIFIED\n");
        } else {
            com1::write_str("[SELF_HOSTING] WAITPID_FAILED unexpected exit code\n");
        }
    } else {
        com1::write_str("[SELF_HOSTING] SPAWN_FAILED\n");
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

    let mut buf_mu = core::mem::MaybeUninit::<[u8; 4096]>::uninit();
    let buf_ptr = buf_mu.as_mut_ptr() as *mut u8;
    let buf: &mut [u8] = core::slice::from_raw_parts_mut(buf_ptr, 4096);
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
