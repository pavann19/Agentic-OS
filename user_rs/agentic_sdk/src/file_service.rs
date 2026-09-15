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
    offset: u32,
}

/// Asks the real file-serving process to write `data` into `inode` at `offset`.
/// Returns a real request id to poll for via `poll_reply`, or `0` on failure/denial.
pub unsafe fn write_file_at(inode: u32, offset: u32, data: &[u8]) -> u64 {
    let req = FileWriteRequest {
        data_vaddr: data.as_ptr() as u64,
        len: data.len() as u32,
        offset,
    };
    let req_vaddr = &req as *const FileWriteRequest as u64;
    syscall2(24, inode as u64, req_vaddr)
}

/// Asks the real file-serving process to write `data` into `inode` at offset 0.
pub unsafe fn write_file(inode: u32, data: &[u8]) -> u64 {
    write_file_at(inode, 0, data)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FsDirEntry {
    pub inode: u32,
    pub file_type: u8,
    pub name_len: u8,
    pub pad: u16,
    pub name: [u8; 56],
}

impl Default for FsDirEntry {
    fn default() -> Self {
        Self {
            inode: 0,
            file_type: 0,
            name_len: 0,
            pad: 0,
            name: [0u8; 56],
        }
    }
}

impl FsDirEntry {
    pub fn name_str(&self) -> &str {
        let len = (self.name_len as usize).min(56);
        core::str::from_utf8(&self.name[..len]).unwrap_or("")
    }
}

pub const FS_OP_LOOKUP: u8 = 2;
pub const FS_OP_READDIR: u8 = 3;
pub const FS_OP_MKDIR: u8 = 4;
pub const FS_OP_CREATE: u8 = 5;
pub const FS_OP_UNLINK: u8 = 6;

pub unsafe fn wait_poll(req_id: u64, out: &mut [u8]) -> u64 {
    loop {
        let ret = poll_reply(req_id, out);
        if ret != u64::MAX {
            return ret;
        }
        syscall1(29, 0); // SYS_YIELD
    }
}

pub unsafe fn lookup(path: &str) -> Option<(u32, u8, u32)> {
    let req_id = write_file_at((FS_OP_LOOKUP as u32) << 24, 0, path.as_bytes());
    if req_id == 0 { return None; }
    let mut out = [0u8; 16];
    let len = wait_poll(req_id, &mut out);
    if len >= 12 {
        let inode = u32::from_le_bytes(out[0..4].try_into().unwrap());
        let file_type = out[4];
        let size = u32::from_le_bytes(out[8..12].try_into().unwrap());
        Some((inode, file_type, size))
    } else {
        None
    }
}

pub unsafe fn mkdir(path: &str) -> bool {
    let req_id = write_file_at((FS_OP_MKDIR as u32) << 24, 0, path.as_bytes());
    if req_id == 0 { return false; }
    let mut out = [0u8; 4];
    let len = wait_poll(req_id, &mut out);
    len >= 4
}

pub unsafe fn create_file(path: &str) -> Option<u32> {
    let req_id = write_file_at((FS_OP_CREATE as u32) << 24, 0, path.as_bytes());
    if req_id == 0 { return None; }
    let mut out = [0u8; 4];
    let len = wait_poll(req_id, &mut out);
    if len >= 4 {
        let ino = u32::from_le_bytes(out[0..4].try_into().unwrap());
        if ino != 0 { Some(ino) } else { None }
    } else {
        None
    }
}

pub unsafe fn unlink(path: &str) -> bool {
    let req_id = write_file_at((FS_OP_UNLINK as u32) << 24, 0, path.as_bytes());
    if req_id == 0 { return false; }
    let mut out = [0u8; 1];
    let len = wait_poll(req_id, &mut out);
    len > 0 && out[0] == 1
}

pub unsafe fn readdir(path: &str, entries: &mut [FsDirEntry]) -> usize {
    let req_id = write_file_at((FS_OP_READDIR as u32) << 24, 0, path.as_bytes());
    if req_id == 0 { return 0; }
    let mut raw_buf = [0u8; 4096];
    let len = wait_poll(req_id, &mut raw_buf) as usize;
    let count = (len / 64).min(entries.len());
    for i in 0..count {
        let slot = &raw_buf[i * 64..(i + 1) * 64];
        let mut entry = FsDirEntry {
            inode: u32::from_le_bytes(slot[0..4].try_into().unwrap()),
            file_type: slot[4],
            name_len: slot[5],
            pad: 0,
            name: [0u8; 56],
        };
        let nlen = (entry.name_len as usize).min(56);
        entry.name[..nlen].copy_from_slice(&slot[8..8 + nlen]);
        entries[i] = entry;
    }
    count
}

pub unsafe fn write_file_path(path: &str, data: &[u8]) -> bool {
    let inode = match lookup(path) {
        Some((ino, _, _)) => ino,
        None => match create_file(path) {
            Some(ino) => ino,
            None => return false,
        },
    };
    let req_id = write_file(inode, data);
    if req_id == 0 { return false; }
    let len = wait_poll(req_id, &mut []);
    len as usize == data.len()
}

pub unsafe fn read_file_path(path: &str, out: &mut [u8]) -> usize {
    let (inode, _, _) = match lookup(path) {
        Some(res) => res,
        None => return 0,
    };
    let req_id = request_file(inode);
    if req_id == 0 { return 0; }
    wait_poll(req_id, out) as usize
}
