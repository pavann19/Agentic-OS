//! Ring-3 side of the real block/file-I/O-for-apps path
//! (`kernel_rs::file_service`) -- lets ANY app (not just a dedicated
//! disk driver) ask for a real file's content, mediated by the kernel
//! over a real IPC round trip to whichever process registered as the
//! file-serving server (`virtio_blk_driver`, today).

use crate::syscall::{syscall1, syscall2};

/// Real request struct for `SYS_FILE_SERVICE_POLL` (syscall 19) -- MUST
/// stay field-for-field identical to `kernel_rs::file_service::
/// FilePollRequest`, the same raw ABI contract every other
/// `INFO_VADDR`-mapped/request struct in this kernel already relies on.
#[repr(C)]
struct FilePollRequest {
    out_vaddr: u64,
    out_max_len: u32,
}

/// Asks the real file-serving process for `inode`'s content. Returns a
/// real request id to poll for via `poll_reply`, or `0` if no server
/// has ever registered (e.g. the QEMU config this kernel booted under
/// never attached a virtio-blk device).
pub unsafe fn request_file(inode: u32) -> u64 {
    syscall1(17, inode as u64)
}

/// Non-blocking poll for `request_id`'s real reply, same discipline as
/// `surface::present`'s own syscall-16 pairing: `u64::MAX` means "not
/// ready yet" (or a stale/unknown id), a real byte count otherwise --
/// that many real bytes were just written into `out_buf` (or written to disk).
pub unsafe fn poll_reply(request_id: u64, out_buf: &mut [u8]) -> u64 {
    let req = FilePollRequest {
        out_vaddr: out_buf.as_mut_ptr() as u64,
        out_max_len: out_buf.len() as u32,
    };
    let req_vaddr = &req as *const FilePollRequest as u64;
    syscall2(19, request_id, req_vaddr)
}

/// Real request struct for `SYS_FILE_SERVICE_WRITE` (syscall 24).
#[repr(C)]
struct FileWriteRequest {
    data_vaddr: u64,
    len: u32,
}

/// Asks the real file-serving process to write `data` into `inode`. Returns a
/// real request id to poll for via `poll_reply`, or `0` if no server
/// has registered or data is invalid.
pub unsafe fn write_file(inode: u32, data: &[u8]) -> u64 {
    let req = FileWriteRequest {
        data_vaddr: data.as_ptr() as u64,
        len: data.len() as u32,
    };
    let req_vaddr = &req as *const FileWriteRequest as u64;
    syscall2(24, inode as u64, req_vaddr)
}
