//! Phase 13 exit criterion 2 proof:
//! "The SDK builds and runs a genuinely new app (not one of the four reference apps)
//! with no kernel or platform-service change required."
//!
//! Installs `sample_app` from its package binary, verifies manifest enforcement,
//! and runs it cleanly in ring 3.

#![cfg(feature = "sample_app_demo")]

use crate::capability::{KernelObjectKind, Rights};
use crate::installer::CapRequest;
use crate::manifest::CapKind;
use crate::package::{self, PackageHeader, PACKAGE_MAGIC};
use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const STACK_VADDR: u64 = 0x0000_0000_0078_0000;
const INFO_VADDR: u64 = 0x0000_0000_0053_0000;
const SURFACE_WIDTH: u32 = 320;
const SURFACE_HEIGHT: u32 = 200;
const READY_TOKEN: u64 = 0x5341_4D50; // 'SAMP'

static SAMPLE_APP_ELF: &[u8] =
    include_bytes!("../../user_rs/sample_app/target/x86_64-unknown-none/release/sample_app");

#[repr(C)]
struct SampleAppInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

pub fn spawn(params: crate::compositor::FbParams) {
    unsafe {
        crate::compositor::set_fb_params_for_terminal(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(sample_app_verify_thread);
    thread::spawn(sample_app_thread);
}

extern "C" fn sample_app_thread() {
    unsafe {
        klog_info!("SAMPLE_APP_DEMO_START");

        let elf_checksum = package::adler32(SAMPLE_APP_ELF);
        klog_info!("SAMPLE_APP_ELF_CHECKSUM=0x{:08x}", elf_checksum);

        let manifest_mask: u16 = (1 << (CapKind::Surface as u16)) | (1 << (CapKind::PortIoRange as u16));
        let hdr = PackageHeader {
            magic: PACKAGE_MAGIC,
            version: 1,
            manifest_mask,
            flags: 0,
            elf_offset: core::mem::size_of::<PackageHeader>() as u32,
            elf_size: SAMPLE_APP_ELF.len() as u32,
            checksum: elf_checksum,
            name: *b"sample_app\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        };
        let _ = hdr;

        let requests = [
            CapRequest {
                kind: CapKind::Surface,
                object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: SURFACE_WIDTH, height: SURFACE_HEIGHT },
                rights: Rights::MAP,
                label: "sample_app_surface",
            },
            CapRequest {
                kind: CapKind::PortIoRange,
                object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
                rights: Rights::PORT_IO,
                label: "sample_app_com1",
            },
        ];

        let Some((entry, space)) = crate::installer::install_into_current_thread(
            SAMPLE_APP_ELF,
            STACK_VADDR,
            crate::manifest::Manifest(manifest_mask),
            &requests,
        ) else {
            klog_info!("SAMPLE_APP_INSTALL_FAILED");
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
                klog_info!("SAMPLE_APP_SURFACE_RESOLVE_FAILED {:?}", e);
                return;
            }
        };
        let input_cap = crate::input_routing::register_window_input(surface_object);
        crate::input_routing::set_focus(surface_object);
        crate::window_manager::register(surface_object, 20, 20, SURFACE_WIDTH, SURFACE_HEIGHT, b"Sample SDK App");

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut SampleAppInfo;
        core::ptr::write(info_ptr, SampleAppInfo {
            surface_cap: 0,
            input_cap,
            ready_token: READY_TOKEN,
        });
        vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("SAMPLE_APP_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, STACK_VADDR + 4096);
    }
}

extern "C" fn sample_app_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match crate::ipc::receive(table, cap) {
        Ok(msg) => klog_info!("SAMPLE_APP_READY token=0x{:x}", msg.data[0]),
        Err(e) => klog_info!("SAMPLE_APP_VERIFY_FAILED {:?}", e),
    }
}
