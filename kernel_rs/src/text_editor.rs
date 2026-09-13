//! Phase 13 deliverable 4: kernel-side spawn code for the second real
//! reference app (`user_rs/text_editor`) -- same real, capability-gated
//! install path as `terminal.rs` (manifest-declared Surface + PortIoRange
//! through `installer.rs`, routed input wired separately through
//! `input_routing.rs`, a real window registered with `window_manager`).
//!
//! Off by default (`text_editor_demo` feature), mutually exclusive with
//! `compositor_demo`/`terminal_demo`: this app also wants sole keyboard
//! focus, for the same reason `terminal.rs`'s own module doc gives.

use crate::capability::{KernelObjectKind, Rights};
use crate::installer::{self, CapRequest};
use crate::manifest::{CapKind, Manifest};
use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const STACK_VADDR: u64 = 0x0000_0000_0072_0000;
const INFO_VADDR: u64 = 0x0000_0000_0052_0000;
const SURFACE_WIDTH: u32 = 448;
const SURFACE_HEIGHT: u32 = 256;
const READY_TOKEN: u64 = 0x7E13_0000;

static TEXT_EDITOR_ELF: &[u8] =
    include_bytes!("../../user_rs/text_editor/target/x86_64-unknown-none/release/text_editor");

#[repr(C)]
struct EditorInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

pub fn spawn(params: crate::compositor::FbParams) {
    unsafe {
        crate::compositor::set_fb_params_for_terminal(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(text_editor_verify_thread);
    thread::spawn(text_editor_thread);
}

extern "C" fn text_editor_thread() {
    unsafe {
        let manifest = Manifest::NONE.allow(CapKind::Surface).allow(CapKind::PortIoRange);
        let requests = [
            CapRequest {
                kind: CapKind::Surface,
                object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: SURFACE_WIDTH, height: SURFACE_HEIGHT },
                rights: Rights::MAP,
                label: "text_editor_surface",
            },
            CapRequest {
                kind: CapKind::PortIoRange,
                object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
                rights: Rights::PORT_IO,
                label: "text_editor_com1",
            },
        ];
        let Some((entry, space)) = installer::install_into_current_thread(TEXT_EDITOR_ELF, STACK_VADDR, manifest, &requests) else {
            klog_info!("TEXT_EDITOR_INSTALL_FAILED");
            return;
        };

        // Same real extra-stack-page fix `terminal.rs`'s own doc
        // explains in full (installer.rs maps exactly one 4KB stack
        // page; this app's own ROWS*COLS grid plus per-row length
        // array pushes real usage past that, and a real, reproduced
        // write #PF landed just above the nominal stack top too).
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
                klog_info!("TEXT_EDITOR_SURFACE_RESOLVE_FAILED {:?}", e);
                return;
            }
        };
        let input_cap = crate::input_routing::register_window_input(surface_object);
        crate::input_routing::set_focus(surface_object);
        crate::window_manager::register(surface_object, 20, 20, SURFACE_WIDTH, SURFACE_HEIGHT, b"Text Editor");

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut EditorInfo;
        core::ptr::write(info_ptr, EditorInfo { surface_cap: 0, input_cap, ready_token: READY_TOKEN });
        vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("TEXT_EDITOR_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, STACK_VADDR + 4096);
    }
}

extern "C" fn text_editor_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match crate::ipc::receive(table, cap) {
        Ok(msg) => klog_info!("TEXT_EDITOR_READY token=0x{:x}", msg.data[0]),
        Err(e) => klog_info!("TEXT_EDITOR_VERIFY_FAILED {:?}", e),
    }
}
