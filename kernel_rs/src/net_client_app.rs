//! Phase 13 deliverable 4: kernel-side spawn code for the fourth real
//! reference app (`user_rs/net_client`) -- same real manifest-gated
//! install path as `terminal.rs`/`text_editor.rs`/`file_manager.rs`, plus
//! the real network service path (`net_service.rs`). Exercises Phase 10's
//! real TCP network stack.

use crate::capability::{KernelObjectKind, Rights, SocketProtocol};
use crate::installer::{self, CapRequest};
use crate::manifest::{CapKind, Manifest};
use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const STACK_VADDR: u64 = 0x0000_0000_0076_0000;
const INFO_VADDR: u64 = 0x0000_0000_0053_0000;
const SURFACE_WIDTH: u32 = 300;
const SURFACE_HEIGHT: u32 = 180;
const READY_TOKEN: u64 = 0x4E45_5431;

pub static NET_CLIENT_ELF: &[u8] =
    include_bytes!("../../user_rs/net_client/target/x86_64-unknown-none/release/net_client");

#[repr(C)]
struct NetClientInfo {
    surface_cap: u32,
    input_cap: u32,
    socket_cap: u32,
    ready_token: u64,
}

pub fn spawn(params: crate::compositor::FbParams) {
    unsafe {
        crate::compositor::set_fb_params_for_terminal(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(net_client_verify_thread);
    thread::spawn(net_client_thread);
}

pub fn spawn_at(x: i32, y: i32) {
    let closure_data = (x, y);
    unsafe {
        NET_CLIENT_POS = closure_data;
    }
    thread::spawn(net_client_thread_at);
}

static mut NET_CLIENT_POS: (i32, i32) = (340, 220);

extern "C" fn net_client_thread() {
    spawn_net_client_inner(20, 20);
}

extern "C" fn net_client_thread_at() {
    let (x, y) = unsafe { NET_CLIENT_POS };
    spawn_net_client_inner(x, y);
}

fn spawn_net_client_inner(x: i32, y: i32) {
    unsafe {
        let manifest = Manifest::NONE
            .allow(CapKind::Surface)
            .allow(CapKind::Socket)
            .allow(CapKind::PortIoRange);
        let requests = [
            CapRequest {
                kind: CapKind::Surface,
                object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: SURFACE_WIDTH, height: SURFACE_HEIGHT },
                rights: Rights::MAP,
                label: "net_client_surface",
            },
            CapRequest {
                kind: CapKind::Socket,
                object_kind: KernelObjectKind::Socket {
                    protocol: SocketProtocol::Tcp,
                    local_port: 53000,
                    remote_ip: [10, 0, 2, 2],
                    remote_port: 80,
                },
                rights: Rights::SEND,
                label: "net_client_socket",
            },
            CapRequest {
                kind: CapKind::PortIoRange,
                object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
                rights: Rights::PORT_IO,
                label: "net_client_com1",
            },
        ];
        let Some((entry, space)) = installer::install_into_current_thread(NET_CLIENT_ELF, STACK_VADDR, manifest, &requests) else {
            klog_info!("NET_CLIENT_INSTALL_FAILED");
            return;
        };

        for extra_page in 1..4u64 {
            let page = pmm::alloc_page();
            vmm::map_page_in(space, STACK_VADDR - extra_page * 4096, page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        }
        for extra_above in 0..2u64 {
            let page = pmm::alloc_page();
            vmm::map_page_in(space, STACK_VADDR + 4096 + extra_above * 4096, page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        }

        let surface_object = match thread::resolve_current_capability(0, Rights::MAP) {
            Ok(cap) => cap.object_id,
            Err(e) => {
                klog_info!("NET_CLIENT_SURFACE_RESOLVE_FAILED {:?}", e);
                return;
            }
        };
        let input_cap = crate::input_routing::register_window_input(surface_object);
        crate::window_manager::register(surface_object, x, y, SURFACE_WIDTH, SURFACE_HEIGHT, b"Net Client");

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut NetClientInfo;
        core::ptr::write(info_ptr, NetClientInfo {
            surface_cap: 0,
            input_cap,
            socket_cap: 1, // Cap 1 is the granted Socket capability
            ready_token: READY_TOKEN,
        });
        vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("NET_CLIENT_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, STACK_VADDR + 4096);
    }
}

extern "C" fn net_client_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match crate::ipc::receive(table, cap) {
        Ok(msg) => klog_info!("NET_CLIENT_READY token=0x{:x}", msg.data[0]),
        Err(e) => klog_info!("NET_CLIENT_VERIFY_FAILED {:?}", e),
    }
}
