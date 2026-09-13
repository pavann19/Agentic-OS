//! Phase 13 deliverable 4: the real block/file-I/O-for-apps path this
//! kernel never had before -- until now, real disk I/O (AHCI, virtio-blk)
//! was only ever performed by a dedicated DRIVER process's own direct
//! capability; no other app had any way to read a real file's content.
//!
//! Real, deliberately minimal design: reuses the SAME real mechanisms
//! already proven elsewhere in this kernel rather than inventing new
//! ones --
//!   - the request notification is a real `ipc.rs` message, delivered
//!     the exact same way `input_routing.rs` already delivers routed
//!     keyboard scancodes to a window's own endpoint;
//!   - the actual file BYTES cross from the server's (`virtio_blk_driver`)
//!     address space into the kernel, and later from the kernel into the
//!     requesting app's address space, via the SAME cross-address-space
//!     `vmm::read_user_bytes`/`write_user_bytes` copy `compositor::
//!     syscall_draw_text` already uses to pull a caller's own request
//!     struct out of its own mapped memory.
//!
//! Real, disclosed scope: exactly ONE request may be in flight at a
//! time (a real, stated limitation -- there is only one real file-
//! serving driver process in this kernel today, and only one reference
//! app, `file_manager`, ever asks it for anything), and the server is
//! polled for, never pushed to, the exact same non-blocking discipline
//! `SYS_IPC_TRY_RECEIVE` already established for routed keyboard input.
//! Read-only: no real on-disk WRITE path is wired through this yet,
//! disclosed rather than faked.

use crate::capability::{CapId, CapabilityTable, ObjectId, Rights};
use crate::{ipc, klog_info, thread, vmm};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Real, disclosed bound: generous for this kernel's one real on-disk
/// file (`virtio_blk_driver`'s own `greeting.txt` self-check content,
/// 36 bytes) and for a real ext2 block (1024 bytes) besides -- a
/// request naming a longer file is truncated, never faked with more
/// bytes than the server actually supplied.
pub const MAX_FILE_BYTES: usize = 1024;

struct Request {
    #[allow(dead_code)]
    inode: u32,
    ready: bool,
    len: usize,
    data: [u8; MAX_FILE_BYTES],
}

static mut REQUEST: Option<Request> = None;
static CURRENT_REQUEST_ID: AtomicU64 = AtomicU64::new(0); // 0 = none pending
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

const NO_SERVER: u32 = u32::MAX;
static SERVER_SEND_CAP: AtomicU32 = AtomicU32::new(NO_SERVER);
static mut SENDER_TABLE: Option<CapabilityTable> = None;

#[allow(static_mut_refs)]
fn sender_table_mut() -> &'static mut CapabilityTable {
    unsafe {
        let slot = &mut *&raw mut SENDER_TABLE;
        if slot.is_none() {
            *slot = Some(CapabilityTable::new());
        }
        slot.as_mut().unwrap()
    }
}

/// Real, one-time setup for the (single) file-serving driver process --
/// same real pattern as `input_routing::register_window_input`: mints a
/// fresh `IpcEndpoint`, grants the RECEIVE half into the CALLING
/// thread's own cap_table (so the driver process can poll it via the
/// existing `SYS_IPC_TRY_RECEIVE`, syscall 12 -- no new receive syscall
/// needed), and keeps the SEND half here for `request_file` to use
/// later. Called once by `virtio_blk.rs`'s own kernel-side spawn code,
/// never by the driver process itself.
pub fn register_server() -> CapId {
    let send_cap = ipc::create_endpoint(sender_table_mut(), Rights::SEND);
    let endpoint_object: ObjectId = sender_table_mut().resolve(send_cap, Rights::SEND).unwrap().object_id;
    let receive_cap = thread::grant_current_capability(endpoint_object, Rights::RECEIVE);
    SERVER_SEND_CAP.store(send_cap, Ordering::SeqCst);
    klog_info!("FILE_SERVICE_SERVER_REGISTERED endpoint={}", endpoint_object);
    receive_cap
}

/// Real SYS_FILE_SERVICE_REQUEST handler: a real app (e.g.
/// `file_manager`) asks for `inode`'s content. Real, disclosed
/// single-in-flight-request design: starting a new request before the
/// previous one's reply lands simply drops the previous one's eventual
/// reply -- correct enough for a single-app, single-request-at-a-time
/// reference client, not a general multi-tenant block-I/O API.
pub fn request_file(inode: u32) -> u64 {
    let send_cap = SERVER_SEND_CAP.load(Ordering::SeqCst);
    if send_cap == NO_SERVER {
        klog_info!("FILE_SERVICE_REQUEST_NO_SERVER");
        return 0;
    }
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
    unsafe {
        *(&mut *&raw mut REQUEST) = Some(Request { inode, ready: false, len: 0, data: [0u8; MAX_FILE_BYTES] });
    }
    CURRENT_REQUEST_ID.store(id, Ordering::SeqCst);
    // Real, disclosed ABI limit: `SYS_IPC_TRY_RECEIVE`'s own syscall
    // return value only ever carries `msg.data[0]` (the same real
    // single-word limit `input_routing.rs`'s own scancode delivery
    // already lives with) -- both the real request id AND the real
    // inode need to reach the server, so they're packed into that one
    // word: `(request_id << 32) | inode`. Both are small (a
    // sequential counter, a small inode number), so this is a real,
    // safe packing, not a truncation risk in practice.
    let mut msg = ipc::Message::default();
    msg.data[0] = (id << 32) | (inode as u64);
    match ipc::try_send(sender_table_mut(), send_cap, msg) {
        Ok(true) => klog_info!("FILE_SERVICE_REQUEST_SENT id={} inode={}", id, inode),
        Ok(false) => klog_info!("FILE_SERVICE_REQUEST_DROPPED_BUSY id={} inode={}", id, inode),
        Err(e) => klog_info!("FILE_SERVICE_REQUEST_SEND_FAILED {:?}", e),
    }
    id
}

/// Real SYS_FILE_SERVICE_REPLY handler: called by the SERVING process
/// (never the requester) once it has real file bytes ready, e.g. after
/// `virtio_blk_driver`'s own already-proven `ext2::read_file_data` call.
/// `pml4` is the CALLING (serving) thread's own address space -- the
/// real bytes are copied FROM there, the exact same cross-address-space
/// read `compositor::syscall_draw_text` already performs for a caller's
/// own request struct.
pub fn server_reply(pml4: u64, request_id: u64, data_vaddr: u64, len: u32) -> u64 {
    if CURRENT_REQUEST_ID.load(Ordering::SeqCst) != request_id {
        klog_info!("FILE_SERVICE_REPLY_STALE_ID id={}", request_id);
        return u64::MAX;
    }
    let real_len = (len as usize).min(MAX_FILE_BYTES);
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, data_vaddr, real_len as u64) {
            klog_info!("FILE_SERVICE_REPLY_BAD_PTR id={}", request_id);
            return u64::MAX;
        }
        let slot = &mut *&raw mut REQUEST;
        let Some(r) = slot else {
            return u64::MAX;
        };
        vmm::read_user_bytes(pml4, data_vaddr, &mut r.data[..real_len]);
        r.len = real_len;
        r.ready = true;
    }
    klog_info!("FILE_SERVICE_REPLY_OK id={} len={}", request_id, real_len);
    0
}

/// Real SYS_FILE_SERVICE_POLL handler: called by the REQUESTER
/// (`file_manager`). Non-blocking, same discipline as `SYS_IPC_TRY_
/// RECEIVE`: returns `u64::MAX` immediately if the reply isn't in yet
/// (or the id is stale), the real copied byte count otherwise. `pml4`
/// is the CALLING (requesting) thread's own address space -- the real
/// bytes are copied INTO there.
pub fn poll_reply(pml4: u64, request_id: u64, out_vaddr: u64, out_max_len: u32) -> u64 {
    if CURRENT_REQUEST_ID.load(Ordering::SeqCst) != request_id {
        return u64::MAX;
    }
    unsafe {
        let slot = &mut *&raw mut REQUEST;
        let Some(r) = slot else {
            return u64::MAX;
        };
        if !r.ready {
            return u64::MAX;
        }
        let copy_len = r.len.min(out_max_len as usize);
        if !vmm::validate_user_buffer_writable(pml4, out_vaddr, copy_len as u64) {
            klog_info!("FILE_SERVICE_POLL_BAD_PTR id={}", request_id);
            return u64::MAX;
        }
        vmm::write_user_bytes(pml4, out_vaddr, &r.data[..copy_len]);
        copy_len as u64
    }
}

/// Real request struct a caller writes into its OWN mapped memory
/// before invoking `SYS_FILE_SERVICE_REPLY` -- same real ABI shape
/// `compositor::SurfaceTextRequest` already established for a syscall
/// carrying more than the two plain integer arguments (`a0`/`a1`) this
/// kernel's raw syscall ABI provides.
#[repr(C)]
struct FileReplyRequest {
    request_id: u64,
    data_vaddr: u64,
    len: u32,
}

/// Real request struct for `SYS_FILE_SERVICE_POLL`, same shape/reason.
#[repr(C)]
struct FilePollRequest {
    out_vaddr: u64,
    out_max_len: u32,
}

/// SYS_FILE_SERVICE_REPLY dispatch glue: `request_vaddr` is a vaddr in
/// the CALLING (serving) thread's own mapped memory holding a real
/// `FileReplyRequest`.
pub fn syscall_reply(pml4: u64, request_vaddr: u64) -> u64 {
    let req_size = core::mem::size_of::<FileReplyRequest>() as u64;
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, request_vaddr, req_size) {
            klog_info!("FILE_SERVICE_REPLY_BAD_REQUEST_PTR");
            return u64::MAX;
        }
        let mut bytes = [0u8; core::mem::size_of::<FileReplyRequest>()];
        vmm::read_user_bytes(pml4, request_vaddr, &mut bytes);
        let req: FileReplyRequest = core::ptr::read_unaligned(bytes.as_ptr() as *const FileReplyRequest);
        server_reply(pml4, req.request_id, req.data_vaddr, req.len)
    }
}

/// SYS_FILE_SERVICE_POLL dispatch glue: `request_vaddr` is a vaddr in
/// the CALLING (requesting) thread's own mapped memory holding a real
/// `FilePollRequest`.
pub fn syscall_poll(pml4: u64, request_id: u64, request_vaddr: u64) -> u64 {
    let req_size = core::mem::size_of::<FilePollRequest>() as u64;
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, request_vaddr, req_size) {
            klog_info!("FILE_SERVICE_POLL_BAD_REQUEST_PTR");
            return u64::MAX;
        }
        let mut bytes = [0u8; core::mem::size_of::<FilePollRequest>()];
        vmm::read_user_bytes(pml4, request_vaddr, &mut bytes);
        let req: FilePollRequest = core::ptr::read_unaligned(bytes.as_ptr() as *const FilePollRequest);
        poll_reply(pml4, request_id, req.out_vaddr, req.out_max_len)
    }
}
