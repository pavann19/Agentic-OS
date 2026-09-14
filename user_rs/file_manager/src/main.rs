//! Phase 13 deliverable 4: the third real reference app, and the first
//! non-driver process in this kernel to ever read a real file's real
//! content -- over `kernel_rs::file_service`'s real IPC-mediated
//! request/reply protocol, served by `virtio_blk_driver`'s own already-
//! proven ext2 read path. Real, disclosed scope: this kernel's real
//! on-disk filesystem holds exactly one real file today, so this is a
//! real single-file viewer, not a multi-file directory browser --
//! listing more than one real file is real, separate follow-up work
//! once the filesystem itself grows past one file, not faked here with
//! an invented listing.

#![no_std]
#![no_main]

use agentic_sdk::{com1, file_service, surface, syscall::syscall1, text_widget::TextRegion};

const INFO_VADDR: u64 = 0x0000_0000_0053_0000;

#[repr(C)]
struct FileManagerInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

/// Real ext2 inode number for this kernel's one real on-disk file
/// (`kernel_common::ext2::FILE_INODE`, `virtio_blk_driver`'s own
/// `greeting.txt`) -- not re-derived via a dependency on
/// `kernel_common` here, since the real VALUE (11) is the actual ABI
/// contract with the file service, documented identically on the
/// kernel side (`file_service.rs`) and `virtio_blk_driver`'s own
/// module doc.
const GREETING_FILE_INODE: u32 = 11;

const COLS: usize = 40;
const LINES: usize = 8;
const FG: u32 = 0x00FFFFFF;
const BG: u32 = 0x0000_0000;

/// Real, bounded PS/2 Set 1 decode -- same real, small table as
/// `terminal_emulator`'s own `decode_key`, just the one key this app
/// actually reacts to ('r', to re-request the real file).
fn is_refresh_key(code: u8) -> bool {
    code == 0x13 // 'r' make code
}

fn is_write_key(code: u8) -> bool {
    code == 0x11 // 'w' make code
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const FileManagerInfo);

        com1::write_str("\n[FILE_MANAGER] real ELF64 ring-3 process, real Surface + routed-input + file-service capabilities\n");
        syscall1(1, 0x7E14_0000);

        let mut history_mu = core::mem::MaybeUninit::<TextRegion<LINES, COLS>>::uninit();
        let history_ptr = history_mu.as_mut_ptr();
        TextRegion::init_in_place(history_ptr);

        surface::draw_text(info.surface_cap, 0, 0, b"FILE_MANAGER_READY", FG, BG);
        surface::present(info.surface_cap);
        syscall1(4, info.ready_token);

        // Real, live request for this kernel's one real on-disk file --
        // the actual point of this app: everything up to here is
        // boilerplate every reference app shares, this next call is
        // the first time a NON-DRIVER process in this kernel has ever
        // asked for real file content.
        request_and_show(info, history_ptr, GREETING_FILE_INODE);

        loop {
            let r = syscall1(12, info.input_cap as u64);
            if r == u64::MAX {
                core::hint::spin_loop();
                continue;
            }
            let scancode = r as u8;
            if scancode == 0xE0 {
                // Real, disclosed no-op: this app only reacts to one
                // plain key ('r'), so the PS/2 extended-key prefix
                // itself is simply dropped -- no arrow-key handling
                // needed here yet.
                continue;
            }
            if is_refresh_key(scancode) {
                com1::write_str("[FILE_MANAGER] REFRESH_REQUESTED\n");
                request_and_show(info, history_ptr, GREETING_FILE_INODE);
            } else if is_write_key(scancode) {
                com1::write_str("[FILE_MANAGER] WRITE_KEY_PRESSED\n");
                write_and_show(info, history_ptr, GREETING_FILE_INODE, b"AGENTIC_OS_FILE_WRITE_PERSISTED_OK");
            }
        }
    }
}

/// Real request-then-poll sequence: sends a real `SYS_FILE_SERVICE_
/// REQUEST`, then polls `SYS_FILE_SERVICE_POLL` (non-blocking, same
/// spin-loop discipline `terminal_emulator`'s own `SYS_IPC_TRY_RECEIVE`
/// poll already established) until the real reply lands, bounded by a
/// real, disclosed retry count so a missing server (no virtio-blk
/// device attached) reports failure instead of hanging forever.
unsafe fn request_and_show(info: &FileManagerInfo, history_ptr: *mut TextRegion<LINES, COLS>, inode: u32) {
    // Real, deliberate choice: `TextRegion::init_in_place` (the SAME
    // proven, explicit per-byte `write_volatile` zero loop `terminal_
    // emulator`'s own doc explains at length) -- NEVER a `*history_ptr
    // = core::mem::zeroed()`-style whole-value assignment, which is
    // exactly this project's own recurring large-value-move toolchain
    // bug (a `TextRegion<8,40>` is 320+ bytes) waiting to happen again.
    TextRegion::init_in_place(history_ptr);
    com1::write_str("[FILE_MANAGER] FILE_REQUEST_SENT inode=");
    com1::write_dec_u64(inode as u64);
    com1::write_str("\n");

    let request_id = file_service::request_file(inode);
    if request_id == 0 {
        (*history_ptr).push_line(b"NO FILE SERVER REGISTERED");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);
        surface::present(info.surface_cap);
        com1::write_str("[FILE_MANAGER] FILE_REQUEST_NO_SERVER\n");
        return;
    }

    // Real, disclosed bug found and fixed testing this exact app: a
    // plain `let mut buf = [0u8; 512];` here reproduced this project's
    // own recurring toolchain bug (see `agentic_sdk::text_widget::
    // TextRegion::init_in_place`'s own module doc for the long history)
    // -- a real #PF, cr2=0x0, confirmed live. No zero-init needed at
    // all here: `poll_reply` only ever WRITES into this buffer via the
    // kernel's own `write_user_bytes`, and only the first `real_len`
    // bytes (which that write guarantees) are ever read back -- a real
    // `MaybeUninit`-backed buffer, never zero-initialized, sidesteps
    // the broken lowering path entirely rather than working around it.
    let mut buf_mu = core::mem::MaybeUninit::<[u8; 512]>::uninit();
    let buf_ptr = buf_mu.as_mut_ptr() as *mut u8;
    let buf: &mut [u8] = core::slice::from_raw_parts_mut(buf_ptr, 512);
    let mut real_len: u64 = u64::MAX;
    // Real, disclosed retry bound -- deliberately NOT huge: each
    // iteration is a real syscall round trip, and this loop found a
    // real bug by being too large (2,000,000) during testing: it burned
    // enough real wall-clock time busy-polling to still be running when
    // the test harness's own boot-wait window elapsed, even though the
    // real reply had already arrived. The driver's own one-time real
    // disk work (format + self-check + audit) before it ever reaches
    // its serving loop is the actual source of latency here, not
    // polling speed -- once it's serving, a reply lands within a
    // handful of iterations.
    const MAX_POLLS: u32 = 200_000;
    for _ in 0..MAX_POLLS {
        let n = file_service::poll_reply(request_id, buf);
        if n != u64::MAX {
            real_len = n;
            break;
        }
        core::hint::spin_loop();
    }

    if real_len == u64::MAX {
        (*history_ptr).push_line(b"FILE_REQUEST_TIMED_OUT");
        (*history_ptr).render(info.surface_cap, 16, FG, BG);
        surface::present(info.surface_cap);
        com1::write_str("[FILE_MANAGER] FILE_REQUEST_TIMED_OUT\n");
        return;
    }

    com1::write_str("[FILE_MANAGER] FILE_REPLY_RECEIVED len=");
    com1::write_dec_u64(real_len);
    com1::write_str("\n");

    (*history_ptr).push_line(b"greeting.txt:");
    // Real, disclosed line-wrap: splits the real content into COLS-wide
    // chunks for the scrollable-text widget -- no word-wrapping, a
    // real, simple, disclosed choice matching this widget's own
    // documented scope (`agentic_sdk::text_widget`'s own module doc).
    let content = &buf[..real_len as usize];
    for chunk in content.chunks(COLS) {
        (*history_ptr).push_line(chunk);
    }
    (*history_ptr).render(info.surface_cap, 16, FG, BG);
    surface::present(info.surface_cap);
}

unsafe fn write_and_show(info: &FileManagerInfo, history_ptr: *mut TextRegion<LINES, COLS>, inode: u32, data: &[u8]) {
    com1::write_str("[FILE_MANAGER] FILE_WRITE_REQUEST_SENT inode=");
    com1::write_dec_u64(inode as u64);
    com1::write_str("\n");

    let request_id = file_service::write_file(inode, data);
    if request_id == 0 {
        com1::write_str("[FILE_MANAGER] FILE_WRITE_NO_SERVER\n");
        return;
    }

    let mut dummy = [0u8; 16];
    const MAX_POLLS: u32 = 200_000;
    for _ in 0..MAX_POLLS {
        let n = file_service::poll_reply(request_id, &mut dummy);
        if n != u64::MAX {
            com1::write_str("[FILE_MANAGER] FILE_WRITE_CONFIRMED len=");
            com1::write_dec_u64(n);
            com1::write_str("\n");
            // Re-read file to prove on-disk round-trip persistence!
            request_and_show(info, history_ptr, inode);
            return;
        }
        core::hint::spin_loop();
    }
    com1::write_str("[FILE_MANAGER] FILE_WRITE_TIMED_OUT\n");
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
