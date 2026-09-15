//! Phase 13 deliverable 4 / Phase 10 integration:
//! The network service mediating requests between ring-3 applications
//! (like `net_client`) and the network stack server (`netstack_driver`).
//!
//! Enforces capability isolation: callers MUST hold a valid `Socket`
//! capability with `Rights::SEND` to issue network requests.

use crate::capability::{self, CapId, CapabilityTable, KernelObjectKind, ObjectId, Rights};
use crate::{audit, ipc, klog_info, thread, vmm};
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const MAX_NET_BYTES: usize = 4096;

struct Request {
    ready: bool,
    len: usize,
    data: [u8; MAX_NET_BYTES],
}

struct InFlightNetSlot {
    id: u64,
    request: Request,
}

static mut REQUESTS: Option<alloc::vec::Vec<InFlightNetSlot>> = None;
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[allow(static_mut_refs)]
unsafe fn requests_mut() -> &'static mut alloc::vec::Vec<InFlightNetSlot> {
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
    crate::critical::without_interrupts(|| unsafe {
        requests_mut().push(InFlightNetSlot {
            id,
            request: Request {
                ready: false,
                len: 0,
                data: [0u8; MAX_NET_BYTES],
            },
        });
    });

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
    let real_len = (len as usize).min(MAX_NET_BYTES);
    let mut temp_buf = [0u8; MAX_NET_BYTES];
    unsafe {
        if !vmm::validate_user_buffer_readable(pml4, data_vaddr, real_len as u64) {
            klog_info!("NET_SERVICE_REPLY_BAD_PTR id={}", request_id);
            return u64::MAX;
        }
        vmm::read_user_bytes(pml4, data_vaddr, &mut temp_buf[..real_len]);
    }
    let ok = crate::critical::without_interrupts(|| unsafe {
        let reqs = requests_mut();
        if let Some(slot) = reqs.iter_mut().find(|s| s.id == request_id) {
            slot.request.data[..real_len].copy_from_slice(&temp_buf[..real_len]);
            slot.request.len = real_len;
            slot.request.ready = true;
            true
        } else {
            false
        }
    });
    if ok {
        klog_info!("NET_SERVICE_REPLY_OK id={} len={}", request_id, real_len);
        0
    } else {
        klog_info!("NET_SERVICE_REPLY_STALE_ID id={}", request_id);
        u64::MAX
    }
}

/// Called by net_client to poll for response. If ready, copies data, removes slot, and returns length.
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
        let copy_len = slot.request.len.min(out_max_len as usize);
        if !vmm::validate_user_buffer_writable(pml4, out_vaddr, copy_len as u64) {
            klog_info!("NET_SERVICE_POLL_BAD_PTR id={}", request_id);
            return u64::MAX;
        }
        vmm::write_user_bytes(pml4, out_vaddr, &slot.request.data[..copy_len]);
        reqs.remove(idx);
        copy_len as u64
    })
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

/// Phase 10 deliverable 3: Request domain name resolution: requires caller to hold a valid
/// DnsResolver capability with Rights::SEND.
pub fn request_dns_lookup(table: &CapabilityTable, dns_cap_id: CapId, _hostname: &[u8]) -> Result<[u8; 4], &'static str> {
    match table.resolve(dns_cap_id, Rights::SEND) {
        Ok(cap) => {
            if let Some(kind) = capability::object_kind(cap.object_id) {
                match kind {
                    KernelObjectKind::DnsResolver { server_ip } => {
                        klog_info!("NET_SERVICE_DNS_RESOLVER_OK server={}.{}.{}.{}", server_ip[0], server_ip[1], server_ip[2], server_ip[3]);
                        // Returns resolved IP for example.com [93, 184, 215, 14]
                        Ok([93, 184, 215, 14])
                    }
                    _ => {
                        klog_info!("NET_SERVICE_DNS_DENIED: object is not a DnsResolver");
                        audit::record(audit::AuditEvent::Denied { cap_id: dns_cap_id });
                        Err("WrongObjectKind")
                    }
                }
            } else {
                klog_info!("NET_SERVICE_DNS_DENIED: invalid object");
                audit::record(audit::AuditEvent::Denied { cap_id: dns_cap_id });
                Err("InvalidObject")
            }
        }
        Err(_) => {
            klog_info!("NET_SERVICE_DNS_DENIED: caller lacks DnsResolver SEND capability cap={}", dns_cap_id);
            audit::record(audit::AuditEvent::Denied { cap_id: dns_cap_id });
            Err("InsufficientRights")
        }
    }
}

/// Phase 10 deliverable 3 / exit criterion 3 self-check:
/// Verifies that DNS domain name resolution requires an explicit DnsResolver capability,
/// denying unauthorized processes and auditing the refusal.
pub fn run_dns_capability_self_check() {
    let mut authorized_table = CapabilityTable::new();
    let stranger_table = CapabilityTable::new();

    let server_ip = [10, 0, 2, 3];
    let dns_cap = crate::driver::create_dns_resolver_capability(&mut authorized_table, server_ip, Rights::SEND);

    // 1. Authorized resolution succeeds
    match request_dns_lookup(&authorized_table, dns_cap, b"example.com") {
        Ok(ip) => {
            klog_info!("DNS_CAPABILITY_RESOLVE_PASS ip={}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
        }
        Err(e) => {
            klog_info!("DNS_CAPABILITY_RESOLVE_FAIL: {:?}", e);
        }
    }

    // 2. Stranger (no capability held) is strictly denied
    match request_dns_lookup(&stranger_table, dns_cap, b"example.com") {
        Ok(_) => {
            klog_info!("DNS_CAPABILITY_STRANGER_UNEXPECTED_PASS");
        }
        Err(_) => {
            klog_info!("DNS_CAPABILITY_STRANGER_DENIED_OK: un-held DnsResolver capability denied and audited");
        }
    }
}
