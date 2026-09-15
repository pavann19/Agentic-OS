#![no_std]
#![no_main]

// Live Agent Bridge Gateway Process
// Freestanding ring-3 process bridging an external LLM / host developer
// over COM2 (0x2F8) to in-guest typed syscalls (SYS_INTROSPECT_WINDOWS,
// SYS_AGENT_UI_ACTION).
// Strictly scoped to transport; kernel syscall handlers enforce capabilities.

const COM1: u16 = 0x3F8;
const COM2: u16 = 0x2F8;

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

fn com1_write_str(s: &str) {
    for b in s.bytes() {
        unsafe {
            while (inb(COM1 + 5) & 0x20) == 0 {
                core::hint::spin_loop();
            }
            if b == b'\n' {
                outb(COM1, b'\r');
                while (inb(COM1 + 5) & 0x20) == 0 {
                    core::hint::spin_loop();
                }
            }
            outb(COM1, b);
        }
    }
}

fn com2_read_byte() -> u8 {
    unsafe {
        while (inb(COM2 + 5) & 0x01) == 0 {
            core::hint::spin_loop();
        }
        inb(COM2)
    }
}

fn com2_write_byte(b: u8) {
    unsafe {
        while (inb(COM2 + 5) & 0x20) == 0 {
            core::hint::spin_loop();
        }
        outb(COM2, b);
    }
}

fn com2_read_exact(buf: &mut [u8]) {
    for slot in buf.iter_mut() {
        *slot = com2_read_byte();
    }
}

fn com2_write_all(buf: &[u8]) {
    for &b in buf {
        com2_write_byte(b);
    }
}

unsafe fn syscall_buf(num: u64, a0: u64, a1: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        in("rax") num,
        in("rdi") a0,
        in("rsi") a1,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

// Request types
const REQ_LIST_WINDOWS: u32 = 1;
const REQ_INJECT_KEY: u32 = 2;
const REQ_INJECT_CLICK: u32 = 3;
const REQ_FOCUS_WINDOW: u32 = 4;
const REQ_QUERY_BOUNDS: u32 = 5;

// Response types
const RESP_OK_WINDOWS: u32 = 0x8000_0001;
const RESP_OK_ACTION: u32 = 0x8000_0002;
const RESP_DENIED: u32 = 0x8000_0003;
const RESP_ERR: u32 = 0x8000_0004;

const MAX_WINDOW_ENTRIES: usize = 16;
const WINDOW_INFO_SIZE: usize = 60;
const BUF_BYTES: usize = MAX_WINDOW_ENTRIES * WINDOW_INFO_SIZE; // 960 bytes

#[repr(C, align(8))]
struct WindowBuf {
    bytes: [u8; BUF_BYTES],
}

static mut BUF: WindowBuf = WindowBuf {
    bytes: [0u8; BUF_BYTES],
};

fn send_response(resp_type: u32, payload: &[u8]) {
    let total_len = 4 + payload.len() as u32;
    // Write total length (u32 LE)
    com2_write_all(&total_len.to_le_bytes());
    // Write response type (u32 LE)
    com2_write_all(&resp_type.to_le_bytes());
    // Write payload bytes
    if !payload.is_empty() {
        com2_write_all(payload);
    }
}

fn send_denied(reason: u32) {
    send_response(RESP_DENIED, &reason.to_le_bytes());
}

fn send_error(code: u32) {
    send_response(RESP_ERR, &code.to_le_bytes());
}

fn send_ok_action(status: u32) {
    send_response(RESP_OK_ACTION, &status.to_le_bytes());
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    com1_write_str("[AGENT_GATEWAY] real ELF64 ring-3 gateway process live, listening on COM2 (0x2F8)\n");

    let mut frame_buf = [0u8; 128];

    loop {
        // Read 4-byte length prefix
        let mut len_bytes = [0u8; 4];
        com2_read_exact(&mut len_bytes);
        let frame_len = u32::from_le_bytes(len_bytes) as usize;

        if frame_len < 4 || frame_len > frame_buf.len() {
            com1_write_str("[AGENT_GATEWAY] ERROR: invalid frame length\n");
            // Consume invalid payload if any
            let drain = if frame_len > 1024 { 0 } else { frame_len };
            for _ in 0..drain {
                com2_read_byte();
            }
            send_error(1); // ErrCode: BadLength
            continue;
        }

        // Read payload bytes
        com2_read_exact(&mut frame_buf[..frame_len]);

        let req_type = u32::from_le_bytes([frame_buf[0], frame_buf[1], frame_buf[2], frame_buf[3]]);

        match req_type {
            REQ_LIST_WINDOWS => {
                com1_write_str("[AGENT_GATEWAY] REQUEST: ListWindows\n");
                let buf_addr = unsafe { core::ptr::addr_of!(BUF.bytes) as u64 };
                let res = unsafe { syscall_buf(32, buf_addr, MAX_WINDOW_ENTRIES as u64) };
                if res == u64::MAX {
                    com1_write_str("[AGENT_GATEWAY] SYS_INTROSPECT_WINDOWS DENIED\n");
                    send_denied(1); // Reason: InsufficientRights / NoSuchCapability
                } else {
                    let count = (res as usize).min(MAX_WINDOW_ENTRIES);
                    com1_write_str("[AGENT_GATEWAY] SYS_INTROSPECT_WINDOWS OK count=");
                    let count_u32 = count as u32;
                    let count_bytes = count_u32.to_le_bytes();
                    let total_payload_len = 4 + count * WINDOW_INFO_SIZE;
                    let total_len = 4 + total_payload_len as u32;

                    // Send response frame
                    com2_write_all(&total_len.to_le_bytes());
                    com2_write_all(&RESP_OK_WINDOWS.to_le_bytes());
                    com2_write_all(&count_bytes);
                    unsafe {
                        let win_bytes = core::slice::from_raw_parts(
                            core::ptr::addr_of!(BUF.bytes) as *const u8,
                            count * WINDOW_INFO_SIZE,
                        );
                        com2_write_all(win_bytes);
                    }
                    com1_write_str(" (windows returned over COM2)\n");
                }
            }
            REQ_INJECT_KEY => {
                if frame_len < 12 {
                    send_error(2); // BadPayload
                    continue;
                }
                let surface = u32::from_le_bytes([frame_buf[4], frame_buf[5], frame_buf[6], frame_buf[7]]);
                let scancode = u32::from_le_bytes([frame_buf[8], frame_buf[9], frame_buf[10], frame_buf[11]]);
                com1_write_str("[AGENT_GATEWAY] REQUEST: InjectKey\n");

                let packed_action = (1u64 << 32) | ((scancode as u64 & 0xFFFF) << 16);
                let res = unsafe { syscall_buf(33, surface as u64, packed_action) };
                if res == 0 {
                    send_ok_action(0);
                } else {
                    send_denied(1);
                }
            }
            REQ_INJECT_CLICK => {
                if frame_len < 16 {
                    send_error(2);
                    continue;
                }
                let surface = u32::from_le_bytes([frame_buf[4], frame_buf[5], frame_buf[6], frame_buf[7]]);
                let x = u32::from_le_bytes([frame_buf[8], frame_buf[9], frame_buf[10], frame_buf[11]]);
                let y = u32::from_le_bytes([frame_buf[12], frame_buf[13], frame_buf[14], frame_buf[15]]);
                com1_write_str("[AGENT_GATEWAY] REQUEST: InjectClick\n");

                let packed_action = (2u64 << 32) | ((x as u64 & 0xFFFF) << 16) | (y as u64 & 0xFFFF);
                let res = unsafe { syscall_buf(33, surface as u64, packed_action) };
                if res == 0 {
                    send_ok_action(0);
                } else {
                    send_denied(1);
                }
            }
            REQ_FOCUS_WINDOW => {
                if frame_len < 8 {
                    send_error(2);
                    continue;
                }
                let surface = u32::from_le_bytes([frame_buf[4], frame_buf[5], frame_buf[6], frame_buf[7]]);
                com1_write_str("[AGENT_GATEWAY] REQUEST: FocusWindow\n");

                let packed_action = 3u64 << 32;
                let res = unsafe { syscall_buf(33, surface as u64, packed_action) };
                if res == 0 {
                    send_ok_action(0);
                } else {
                    send_denied(1);
                }
            }
            REQ_QUERY_BOUNDS => {
                if frame_len < 8 {
                    send_error(2);
                    continue;
                }
                let surface = u32::from_le_bytes([frame_buf[4], frame_buf[5], frame_buf[6], frame_buf[7]]);
                com1_write_str("[AGENT_GATEWAY] REQUEST: QueryBounds\n");

                let packed_action = 4u64 << 32;
                let res = unsafe { syscall_buf(33, surface as u64, packed_action) };
                if res == 0 {
                    send_ok_action(0);
                } else {
                    send_denied(1);
                }
            }
            _ => {
                com1_write_str("[AGENT_GATEWAY] Unknown request type\n");
                send_error(99); // UnknownType
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
