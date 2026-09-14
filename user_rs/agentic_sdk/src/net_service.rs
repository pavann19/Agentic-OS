//! Ring-3 client side of the network service (`kernel_rs::net_service`) --
//! lets capability-holding apps (`CapKind::Socket` with `Rights::SEND`)
//! issue HTTP fetch requests over the kernel to `netstack_driver`.

use crate::syscall::{syscall1, syscall2};

#[repr(C)]
struct NetPollRequest {
    out_vaddr: u64,
    out_max_len: u32,
}

/// Issues an HTTP fetch request using a granted Socket capability.
/// Returns a request_id to poll with `poll_reply`, or 0 on failure/denial.
pub unsafe fn request_fetch(socket_cap: u32) -> u64 {
    syscall1(26, socket_cap as u64)
}

/// Polls for the network response. Returns number of bytes received, or
/// u64::MAX if not yet ready.
pub unsafe fn poll_reply(request_id: u64, out_buf: &mut [u8]) -> u64 {
    let req = NetPollRequest {
        out_vaddr: out_buf.as_mut_ptr() as u64,
        out_max_len: out_buf.len() as u32,
    };
    let req_vaddr = &req as *const NetPollRequest as u64;
    syscall2(28, request_id, req_vaddr)
}
