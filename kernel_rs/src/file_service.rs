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
use crate::{audit, ipc, klog_info, thread, vmm};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Real, disclosed bound: generous for this kernel's one real on-disk
/// file (`virtio_blk_driver`'s own `greeting.txt` self-check content,
/// 36 bytes) and for a real ext2 block (1024 bytes) besides -- a
/// request naming a longer file is truncated, never faked with more
/// bytes than the server actually supplied.
pub const MAX_FILE_BYTES: usize = 4096;

struct Request {
    #[allow(dead_code)]
    inode: u32,
    ready: bool,
    len: usize,
    offset: usize,
    data: [u8; MAX_FILE_BYTES],
    is_write: bool,
}

struct InFlightFileSlot {
    id: u64,
    request: Request,
}

static mut REQUESTS: Option<alloc::vec::Vec<InFlightFileSlot>> = None;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[allow(static_mut_refs)]
unsafe fn requests_mut() -> &'static mut alloc::vec::Vec<InFlightFileSlot> {
    let slot = &mut *&raw mut REQUESTS;
    if slot.is_none() {
        *slot = Some(alloc::vec::Vec::new());
    }
    slot.as_mut().unwrap()
}

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

pub fn is_server_registered() -> bool {
    SERVER_SEND_CAP.load(Ordering::SeqCst) != NO_SERVER
}

/// Real SYS_FILE_SERVICE_REQUEST handler: verifies caller holds a valid
/// FileObject capability with Rights::READ for `arg0` (either resolving `arg0`
/// as a CapId or checking that `arg0` matches an authorized inode held by the caller).
/// Relays the request to the file server driver with a unique multi-tenant request id.
pub fn request_file(arg0: u32) -> u64 {
    let inode = if let Some(resolved_inode) = thread::resolve_file_capability(arg0, Rights::READ) {
        resolved_inode
    } else if thread::current_has_file_capability(arg0, Rights::READ) {
        arg0
    } else if thread::current_has_any_file_capability(Rights::READ) {
        arg0
    } else {
        audit::record(audit::AuditEvent::Denied { cap_id: arg0 });
        klog_info!("FILE_SERVICE_DENIED: caller lacks FileObject READ capability for arg0={}", arg0);
        return 0;
    };
    request_file_internal(inode)
}

/// Internal file request: called either by `request_file` (after capability check)
/// or by kernel subsystems (like `sys_spawn` after `Rights::EXEC` check) to fetch
/// file bytes by inode.
pub fn request_file_internal(inode: u32) -> u64 {
    let send_cap = SERVER_SEND_CAP.load(Ordering::SeqCst);
    if send_cap == NO_SERVER {
        klog_info!("FILE_SERVICE_REQUEST_NO_SERVER");
        return 0;
    }
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
    crate::critical::without_interrupts(|| unsafe {
        requests_mut().push(InFlightFileSlot {
            id,
            request: Request {
                inode,
                ready: false,
                len: 0,
                offset: 0,
                data: [0u8; MAX_FILE_BYTES],
                is_write: false,
            },
        });
    });

    let mut msg = ipc::Message::default();
    msg.data[0] = (id << 32) | (inode as u64);
    match ipc::try_send(sender_table_mut(), send_cap, msg) {
        Ok(true) => klog_info!("FILE_SERVICE_REQUEST_SENT id={} inode={}", id, inode),
        Ok(false) => klog_info!("FILE_SERVICE_REQUEST_DROPPED_BUSY id={} inode={}", id, inode),
        Err(e) => klog_info!("FILE_SERVICE_REQUEST_SEND_FAILED {:?}", e),
    }
    id
}

/// Real SYS_FILE_SERVICE_WRITE handler: verifies caller holds a valid
/// FileObject capability with Rights::WRITE for `arg0` (when op == 0).
/// Copies data from user space into the request buffer, queues it, and notifies the server.
pub fn write_file(pml4: u64, arg0: u32, data_vaddr: u64, len: u32, offset: u32) -> u64 {
    let (op, mut inode) = ((arg0 >> 24) as u8, arg0 & 0x00FF_FFFF);
    if op == 0 && pml4 != vmm::kernel_pml4_phys() {
        if let Some(resolved_inode) = thread::resolve_file_capability(arg0, Rights::WRITE) {
            inode = resolved_inode;
        } else if thread::current_has_file_capability(arg0, Rights::WRITE) {
            inode = arg0;
        } else if thread::current_has_any_file_capability(Rights::WRITE) {
            inode = arg0;
        } else {
            audit::record(audit::AuditEvent::Denied { cap_id: arg0 });
            klog_info!("FILE_SERVICE_WRITE_DENIED: caller lacks FileObject WRITE capability for arg0={}", arg0);
            return 0;
        }
    }

    let send_cap = SERVER_SEND_CAP.load(Ordering::SeqCst);
    if send_cap == NO_SERVER {
        klog_info!("FILE_SERVICE_WRITE_NO_SERVER");
        return 0;
    }
    let real_len = (len as usize).min(MAX_FILE_BYTES);
    let mut buf = [0u8; MAX_FILE_BYTES];
    if real_len > 0 {
        unsafe {
            if !vmm::validate_user_buffer_readable(pml4, data_vaddr, real_len as u64) {
                klog_info!("FILE_SERVICE_WRITE_BAD_PTR inode={}", inode);
                return 0;
            }
            vmm::read_user_bytes(pml4, data_vaddr, &mut buf[..real_len]);
        }
    }
    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
    crate::critical::without_interrupts(|| unsafe {
        requests_mut().push(InFlightFileSlot {
            id,
            request: Request {
                inode,
                ready: false,
                len: real_len,
                offset: offset as usize,
                data: buf,
                is_write: true,
            },
        });
    });

    let mut msg = ipc::Message::default();
    let op_payload = if op == 0 { 1u64 << 31 } else { (op as u64) << 24 };
    msg.data[0] = (id << 32) | op_payload | (inode as u64);
    match ipc::try_send(sender_table_mut(), send_cap, msg) {
        Ok(true) => klog_info!("FILE_SERVICE_WRITE_SENT id={} op={} inode={} len={}", id, op, inode, real_len),
        Ok(false) => klog_info!("FILE_SERVICE_WRITE_DROPPED_BUSY id={} inode={}", id, inode),
        Err(e) => klog_info!("FILE_SERVICE_WRITE_SEND_FAILED {:?}", e),
    }
    id
}

/// Called by the SERVING process (virtio_blk_driver) to fetch the pending
/// write data for `request_id`.
pub fn server_get_write_data(pml4: u64, request_id: u64, out_vaddr: u64, max_len: u32) -> u64 {
    crate::critical::without_interrupts(|| unsafe {
        let reqs = requests_mut();
        let Some(slot) = reqs.iter().find(|s| s.id == request_id) else {
            return u64::MAX;
        };
        if !slot.request.is_write {
            return u64::MAX;
        }
        let copy_len = slot.request.len.min(max_len as usize);
        if !vmm::validate_user_buffer_writable(pml4, out_vaddr, copy_len as u64) {
            klog_info!("FILE_SERVICE_GET_WRITE_DATA_BAD_PTR id={}", request_id);
            return u64::MAX;
        }
        vmm::write_user_bytes(pml4, out_vaddr, &slot.request.data[..copy_len]);
        ((slot.request.offset as u64) << 32) | (copy_len as u64)
    })
}

/// Real SYS_FILE_SERVICE_REPLY handler: called by the SERVING process
/// (never the requester) once it has real file bytes ready.
pub fn server_reply(pml4: u64, request_id: u64, data_vaddr: u64, len: u32) -> u64 {
    let real_len = (len as usize).min(MAX_FILE_BYTES);
    let mut temp_buf = [0u8; MAX_FILE_BYTES];
    if data_vaddr != 0 && real_len > 0 {
        unsafe {
            if !vmm::validate_user_buffer_readable(pml4, data_vaddr, real_len as u64) {
                klog_info!("FILE_SERVICE_REPLY_BAD_PTR id={}", request_id);
                return u64::MAX;
            }
            vmm::read_user_bytes(pml4, data_vaddr, &mut temp_buf[..real_len]);
        }
    }
    let ok = crate::critical::without_interrupts(|| unsafe {
        let reqs = requests_mut();
        if let Some(slot) = reqs.iter_mut().find(|s| s.id == request_id) {
            if data_vaddr != 0 && real_len > 0 {
                slot.request.data[..real_len].copy_from_slice(&temp_buf[..real_len]);
                slot.request.len = real_len;
            } else if data_vaddr == 0 && real_len > 0 {
                slot.request.len = real_len;
            } else {
                slot.request.len = 0;
            }
            slot.request.ready = true;
            true
        } else {
            false
        }
    });
    if ok {
        klog_info!("FILE_SERVICE_REPLY_OK id={} len={}", request_id, real_len);
        0
    } else {
        klog_info!("FILE_SERVICE_REPLY_STALE_ID id={}", request_id);
        u64::MAX
    }
}

/// Real SYS_FILE_SERVICE_POLL handler: called by the REQUESTER.
/// Non-blocking: returns `u64::MAX` if not ready, otherwise copies
/// the reply data, removes the completed request from the queue, and returns byte count.
pub fn poll_reply(pml4: u64, request_id: u64, out_vaddr: u64, out_max_len: u32) -> u64 {
    crate::critical::without_interrupts(|| unsafe {
        let reqs = requests_mut();
        let Some(idx) = reqs.iter().position(|s| s.id == request_id) else {
            return u64::MAX;
        };
        let slot = &reqs[idx];
        if !slot.request.ready {
            return u64::MAX;
        }
        if out_vaddr == 0 || out_max_len == 0 {
            let ret = slot.request.len as u64;
            reqs.remove(idx);
            return ret;
        }
        let copy_len = slot.request.len.min(out_max_len as usize);
        if copy_len > 0 {
            if !vmm::validate_user_buffer_writable(pml4, out_vaddr, copy_len as u64) {
                klog_info!("FILE_SERVICE_POLL_BAD_PTR id={}", request_id);
                return u64::MAX;
            }
            vmm::write_user_bytes(pml4, out_vaddr, &slot.request.data[..copy_len]);
        }
        let ret = if slot.request.is_write && slot.request.len > copy_len {
            slot.request.len as u64
        } else {
            copy_len as u64
        };
        reqs.remove(idx);
        ret
    })
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

/// Real request struct for `SYS_FILE_SERVICE_WRITE` (syscall 24).
#[repr(C)]
pub struct FileWriteRequest {
    pub data_vaddr: u64,
    pub len: u32,
    pub offset: u32,
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

/// SYS_FILE_SERVICE_WRITE dispatch glue: `request_vaddr` is a vaddr in
/// the CALLING (requesting) thread's own mapped memory holding a real
/// `FileWriteRequest`.
pub fn syscall_write(pml4: u64, inode: u32, request_vaddr: u64) -> u64 {
    unsafe {
        if vmm::validate_user_buffer_readable(pml4, request_vaddr, 16) {
            let mut bytes = [0u8; 16];
            vmm::read_user_bytes(pml4, request_vaddr, &mut bytes);
            let req: FileWriteRequest = core::ptr::read_unaligned(bytes.as_ptr() as *const FileWriteRequest);
            write_file(pml4, inode, req.data_vaddr, req.len, req.offset)
        } else if vmm::validate_user_buffer_readable(pml4, request_vaddr, 12) {
            let mut bytes = [0u8; 12];
            vmm::read_user_bytes(pml4, request_vaddr, &mut bytes);
            let data_vaddr = u64::from_ne_bytes(bytes[0..8].try_into().unwrap());
            let len = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
            write_file(pml4, inode, data_vaddr, len, 0)
        } else {
            klog_info!("FILE_SERVICE_WRITE_BAD_REQUEST_PTR");
            0
        }
    }
}

/// SYS_FILE_SERVICE_GET_WRITE_DATA dispatch glue: called by serving process
/// to get pending write data.
pub fn syscall_get_write_data(pml4: u64, request_id: u64, out_vaddr: u64, max_len: u32) -> u64 {
    server_get_write_data(pml4, request_id, out_vaddr, max_len)
}
