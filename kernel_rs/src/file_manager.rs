//! Phase 13 deliverable 4: kernel-side spawn code for the third real
//! reference app (`user_rs/file_manager`) -- same real manifest-gated
//! install path as `terminal.rs`/`text_editor.rs`, plus the real
//! block/file-I/O-for-apps path (`file_service.rs`): this is the first
//! reference app that also needs a real `virtio-blk` device actually
//! attached (see `scripts/test-file-manager.ps1`) for its own on-screen
//! content to be real, rather than "no server registered".
//!
//! Off by default (`file_manager_demo` feature), mutually exclusive
//! with `compositor_demo`/`terminal_demo`/`text_editor_demo`: same
//! sole-keyboard-focus reasoning as those.

use crate::capability::{KernelObjectKind, Rights};
use crate::installer::{self, CapRequest};
use crate::manifest::{CapKind, Manifest};
use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const STACK_VADDR: u64 = 0x0000_0000_0074_0000;
const INFO_VADDR: u64 = 0x0000_0000_0053_0000;
const SURFACE_WIDTH: u32 = 320;
const SURFACE_HEIGHT: u32 = 200;
const READY_TOKEN: u64 = 0x7E14_0000;

static FILE_MANAGER_ELF: &[u8] =
    include_bytes!("../../user_rs/file_manager/target/x86_64-unknown-none/release/file_manager");

#[repr(C)]
struct FileManagerInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

pub fn spawn(params: crate::compositor::FbParams) {
    unsafe {
        crate::compositor::set_fb_params_for_terminal(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(file_manager_verify_thread);
    thread::spawn(file_manager_thread);
}

pub fn spawn_at(x: i32, y: i32) {
    unsafe {
        FILE_MANAGER_POS = (x, y);
    }
    thread::spawn(file_manager_thread_at);
}

static mut FILE_MANAGER_POS: (i32, i32) = (20, 50);

extern "C" fn file_manager_thread_at() {
    let (x, y) = unsafe { FILE_MANAGER_POS };
    spawn_file_manager_inner(x, y);
}

extern "C" fn file_manager_thread() {
    spawn_file_manager_inner(20, 50);
}

fn spawn_file_manager_inner(x: i32, y: i32) {
    unsafe {
        let manifest = Manifest::NONE
            .allow(CapKind::Surface)
            .allow(CapKind::PortIoRange)
            .allow(CapKind::FileObject);
        let requests = [
            CapRequest {
                kind: CapKind::Surface,
                object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: SURFACE_WIDTH, height: SURFACE_HEIGHT },
                rights: Rights::MAP,
                label: "file_manager_surface",
            },
            CapRequest {
                kind: CapKind::PortIoRange,
                object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
                rights: Rights::PORT_IO,
                label: "file_manager_com1",
            },
            CapRequest {
                kind: CapKind::FileObject,
                object_kind: KernelObjectKind::FileObject { inode: 11 },
                rights: Rights::READ.union(Rights::WRITE),
                label: "file_manager_file",
            },
        ];
        let Some((entry, space)) = installer::install_into_current_thread(FILE_MANAGER_ELF, STACK_VADDR, manifest, &requests) else {
            klog_info!("FILE_MANAGER_INSTALL_FAILED");
            return;
        };

        // Same real extra-stack-page fix `terminal.rs`'s own doc
        // explains in full.
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
                klog_info!("FILE_MANAGER_SURFACE_RESOLVE_FAILED {:?}", e);
                return;
            }
        };
        let input_cap = crate::input_routing::register_window_input(surface_object);
        crate::input_routing::set_focus(surface_object);
        crate::window_manager::register(surface_object, x, y, SURFACE_WIDTH, SURFACE_HEIGHT, b"File Manager");

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut FileManagerInfo;
        core::ptr::write(info_ptr, FileManagerInfo { surface_cap: 0, input_cap, ready_token: READY_TOKEN });
        vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("FILE_MANAGER_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, STACK_VADDR + 4096);
    }
}

extern "C" fn file_manager_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match crate::ipc::receive(table, cap) {
        Ok(msg) => klog_info!("FILE_MANAGER_READY token=0x{:x}", msg.data[0]),
        Err(e) => klog_info!("FILE_MANAGER_VERIFY_FAILED {:?}", e),
    }
}
