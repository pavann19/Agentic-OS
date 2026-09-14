//! Phase 13 deliverable 4 / Phase 10 integration:
//! The network service mediating requests between ring-3 applications
//! (like `net_client`) and the network stack server (`netstack_driver`).
//!
//! Enforces capability isolation: callers MUST hold a valid `Socket`
//! capability with `Rights::SEND` to issue network requests.

use crate::capability::{self, CapId, CapabilityTable, KernelObjectKind, ObjectId, Rights};
use crate::{audit, ipc, klog_info, thread, vmm};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const MAX_NET_BYTES: usize = 1024;

struct Request {
    ready: bool,
    len: usize,
    data: [u8; MAX_NET_BYTES],
}

static mut REQUEST: Option<Request> = None;
static CURRENT_REQUEST_ID: AtomicU64 = AtomicU64::new(0);
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

pub fn register_server() -> CapId {
    let send_cap = ipc::create_endpoint(sender_table_mut(), Rights::SEND);
    let endpoint_object: ObjectId = sender_table_mut().resolve(send_cap, Rights::SEND).unwrap().object_id;
    let receive_cap = thread::grant_current_capability(endpoint_object, Rights::RECEIVE);
    SERVER_SEND_CAP.store(send_cap, Ordering::SeqCst);
    klog_info!("NET_SERVICE_SERVER_REGISTERED endpoint={}", endpoint_object);
    receive_cap
}

/// Request an HTTP fetch: requires caller to hold a valid Socket capability
/// with Rights::SEND.
pub fn request_fetch(socket_cap_id: CapId) -> u64 {
    let send_cap = SERVER_SEND_CAP.load(Ordering::SeqCst);
    if send_cap == NO_SERVER {
        klog_info!("NET_SERVICE_REQUEST_NO_SERVER");
        return 0;
    }

    // Capability verification: caller MUST hold a Socket capability with SEND right
    match thread::resolve_current_capability(socket_cap_id, Rights::SEND) {
        Ok(cap) => {
            if let Some(kind) = capability::object_kind(cap.object_id) {
                match kind {
                    KernelObjectKind::Socket { .. } => {
                        klog_info!("NET_SERVICE_CAPABILITY_VERIFIED socket_cap={}", socket_cap_id);
                    }
                    _ => {
                        klog_info!("NET_SERVICE_DENIED: object is not a Socket");
                        audit::record(audit::AuditEvent::Denied { cap_id: socket_cap_id });
                        return 0;
                    }
                }
            } else {
                klog_info!("NET_SERVICE_DENIED: invalid object");
                audit::record(audit::AuditEvent::Denied { cap_id: socket_cap_id });
                return 0;
            }
        }
        Err(_) => {
            klog_info!("NET_SERVICE_DENIED: caller lacks Socket SEND capability cap={}", socket_cap_id);
            return 0;
        }
    }

    let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::SeqCst);
    unsafe {
        *(&mut *&raw mut REQUEST) = Some(Request {
            ready: false,
            len: 0,
            data: [0u8; MAX_NET_BYTES],
        });
    }
    CURRENT_REQUEST_ID.store(id, Ordering::SeqCst);

    let mut msg = ipc::Message::default();
    msg.data[0] = (id << 32) | 80; // default HTTP port 80
    match ipc::try_send(sender_table_mut(), send_cap, msg) {
        Ok(true) => klog_info!("NET_SERVICE_REQUEST_SENT id={}", id),
        Ok(false) => klog_info!("NET_SERVICE_REQUEST_DROPPED_BUSY id={}", id),
        Err(e) => klog_info!("NET_SERVICE_REQUEST_SEND_FAILED {:?}", e),
    }
    id
}

/// Called by netstack_driver to provide HTTP response bytes
pub fn server_reply(pml4: u64, request_id: u64, data_vaddr: u64, len: u32) -> u64 {
    if CURRENT_REQUEST_ID.load(Ordering::SeqCst) != request_id {
        klog_info!("NET_SERVICE_REPLY_STALE_ID id={}", request_id);
        return u64::MAX;
    }
    let real_len = (len as usize).min(MAX_NET_BYTES);
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, data_vaddr, real_len as u64) {
            klog_info!("NET_SERVICE_REPLY_BAD_PTR id={}", request_id);
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
    klog_info!("NET_SERVICE_REPLY_OK id={} len={}", request_id, real_len);
    0
}

/// Called by net_client to poll for response
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
            klog_info!("NET_SERVICE_POLL_BAD_PTR id={}", request_id);
            return u64::MAX;
        }
        vmm::write_user_bytes(pml4, out_vaddr, &r.data[..copy_len]);
        copy_len as u64
    }
}

#[repr(C)]
struct NetReplyRequest {
    request_id: u64,
    data_vaddr: u64,
    len: u32,
}

#[repr(C)]
struct NetPollRequest {
    out_vaddr: u64,
    out_max_len: u32,
}

pub fn syscall_request(socket_cap_id: CapId) -> u64 {
    request_fetch(socket_cap_id)
}

pub fn syscall_reply(pml4: u64, request_vaddr: u64) -> u64 {
    let req_size = core::mem::size_of::<NetReplyRequest>() as u64;
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, request_vaddr, req_size) {
            klog_info!("NET_SERVICE_REPLY_BAD_REQUEST_PTR");
            return u64::MAX;
        }
        let mut bytes = [0u8; core::mem::size_of::<NetReplyRequest>()];
        vmm::read_user_bytes(pml4, request_vaddr, &mut bytes);
        let req: NetReplyRequest = core::ptr::read_unaligned(bytes.as_ptr() as *const NetReplyRequest);
        server_reply(pml4, req.request_id, req.data_vaddr, req.len)
    }
}

pub fn syscall_poll(pml4: u64, request_id: u64, request_vaddr: u64) -> u64 {
    let req_size = core::mem::size_of::<NetPollRequest>() as u64;
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, request_vaddr, req_size) {
            klog_info!("NET_SERVICE_POLL_BAD_REQUEST_PTR");
            return u64::MAX;
        }
        let mut bytes = [0u8; core::mem::size_of::<NetPollRequest>()];
        vmm::read_user_bytes(pml4, request_vaddr, &mut bytes);
        let req: NetPollRequest = core::ptr::read_unaligned(bytes.as_ptr() as *const NetPollRequest);
        poll_reply(pml4, request_id, req.out_vaddr, req.out_max_len)
    }
}
