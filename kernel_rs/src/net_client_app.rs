//! Phase 13 deliverable 4: kernel-side spawn code for the fourth real
//! reference app (`user_rs/net_client`) -- same real manifest-gated
//! install path as `terminal.rs`/`text_editor.rs`/`file_manager.rs`, plus
//! the real network service path (`net_service.rs`). Exercises Phase 10's
//! real TCP network stack.
//!
//! Spawn hardening setup: this module is also responsible for writing the
//! `child_proc` ELF binary to disk at `/bin/child` before ring 3 ever runs,
//! so that `net_client`'s own self-hosting groundwork test can spawn it BY
//! PATH through the now-real `sys_spawn` → FS_OP_LOOKUP chain.

use crate::capability::{KernelObjectKind, Rights, SocketProtocol};
use crate::installer::{self, CapRequest};
use crate::manifest::{CapKind, Manifest};
use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const STACK_VADDR: u64 = 0x0000_0000_0076_0000;
const INFO_VADDR: u64 = 0x0000_0000_0053_0000;
const SURFACE_WIDTH: u32 = 300;
const SURFACE_HEIGHT: u32 = 180;
const READY_TOKEN: u64 = 0x4E45_5431;

/// The child_proc ELF binary embedded at kernel build time. Written to
/// `/bin/child` on the real ext2 filesystem by `write_child_proc_to_disk`
/// before `net_client` enters ring 3, so `sys_spawn` can resolve it by path
/// via the now-real FS_OP_LOOKUP → inode → file-read chain (1960b).
pub static CHILD_PROC_ELF: &[u8] =
    include_bytes!("../../user_rs/child_proc/target/x86_64-unknown-none/release/child_proc");

/// The helper_proc ELF binary embedded at kernel build time. Written to
/// `/bin/helper` on the real ext2 filesystem by `write_child_proc_to_disk`
/// to prove that path resolution is genuinely generic and not pattern-matched.
pub static HELPER_PROC_ELF: &[u8] =
    include_bytes!("../../user_rs/helper_proc/target/x86_64-unknown-none/release/helper_proc");

pub static NET_CLIENT_ELF: &[u8] =
    include_bytes!("../../user_rs/net_client/target/x86_64-unknown-none/release/net_client");

fn write_elf_file(path: &[u8], elf_bytes: &[u8]) {
    let path_str = core::str::from_utf8(path).unwrap_or("");
    klog_info!("WRITE_ELF_DISK path={} bytes={}", path_str, elf_bytes.len());

    let create_id = crate::file_service::write_file(
        crate::vmm::kernel_pml4_phys(),
        (5u32) << 24, // FS_OP_CREATE = 5
        path.as_ptr() as u64,
        path.len() as u32,
        0,
    );
    let mut file_inode = 0u32;
    if create_id != 0 {
        let mut out = [0u8; 4];
        for _ in 0..5_000_000u32 {
            let n = crate::file_service::poll_reply(
                crate::vmm::kernel_pml4_phys(), create_id, out.as_mut_ptr() as u64, 4);
            if n != u64::MAX {
                if n >= 4 {
                    file_inode = u32::from_le_bytes(out[0..4].try_into().unwrap_or([0u8; 4]));
                }
                break;
            }
            unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
        }
    }

    if file_inode == 0 {
        let lookup_id = crate::file_service::write_file(
            crate::vmm::kernel_pml4_phys(),
            (2u32) << 24, // FS_OP_LOOKUP = 2
            path.as_ptr() as u64,
            path.len() as u32,
            0,
        );
        if lookup_id != 0 {
            let mut out = [0u8; 16];
            for _ in 0..5_000_000u32 {
                let n = crate::file_service::poll_reply(
                    crate::vmm::kernel_pml4_phys(), lookup_id, out.as_mut_ptr() as u64, 16);
                if n != u64::MAX {
                    if n >= 4 {
                        file_inode = u32::from_le_bytes(out[0..4].try_into().unwrap_or([0u8; 4]));
                    }
                    break;
                }
                unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
            }
        }
    }

    if file_inode == 0 {
        klog_info!("ELF_DISK_SETUP_NO_INODE: {} create+lookup failed", path_str);
        return;
    }

    const CHUNK: usize = crate::file_service::MAX_FILE_BYTES;
    let total = elf_bytes.len();
    let mut offset = 0usize;
    while offset < total {
        let end = (offset + CHUNK).min(total);
        let chunk = &elf_bytes[offset..end];
        let write_id = crate::file_service::write_file(
            crate::vmm::kernel_pml4_phys(),
            file_inode,
            chunk.as_ptr() as u64,
            chunk.len() as u32,
            offset as u32,
        );
        if write_id != 0 {
            for _ in 0..5_000_000u32 {
                let n = crate::file_service::poll_reply(
                    crate::vmm::kernel_pml4_phys(), write_id, 0u64, 0);
                if n != u64::MAX { break; }
                unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
            }
        }
        offset = end;
    }
    klog_info!("ELF_DISK_SETUP_OK path={} inode={} bytes={}", path_str, file_inode, total);
}

/// Kernel-side setup: write the test ELF binaries (`/bin/child` and `/bin/helper`)
/// to disk via the existing `file_service` write path so that `sys_spawn`'s
/// real FS_OP_LOOKUP → file-read chain can find and load them by path. Creates
/// `/bin` if needed, then writes both binaries.
pub fn write_child_proc_to_disk() {
    klog_info!("BINARIES_DISK_SETUP_WAIT_SERVER");
    for _ in 0..10_000_000u32 {
        if crate::file_service::is_server_registered() {
            break;
        }
        unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
    }

    // mkdir /bin (may already exist; ignored on failure)
    let mkdir_id = crate::file_service::write_file(
        crate::vmm::kernel_pml4_phys(),
        (4u32) << 24, // FS_OP_MKDIR = 4
        b"/bin".as_ptr() as u64,
        4,
        0,
    );
    if mkdir_id != 0 {
        let mut _out = [0u8; 4];
        for _ in 0..5_000_000u32 {
            let n = crate::file_service::poll_reply(
                crate::vmm::kernel_pml4_phys(), mkdir_id, _out.as_mut_ptr() as u64, 4);
            if n != u64::MAX { break; }
            unsafe { core::arch::asm!("sti", "hlt", "cli", options(nomem, nostack)) };
        }
    }

    write_elf_file(b"/bin/child", CHILD_PROC_ELF);
    write_elf_file(b"/bin/helper", HELPER_PROC_ELF);
}

#[repr(C)]
struct NetClientInfo {
    surface_cap: u32,
    input_cap: u32,
    socket_cap: u32,
    ready_token: u64,
    file_cap: u32,
    save_to_disk: u32,
    target_inode: u32,
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
    spawn_net_client_inner(20, 50);
}

extern "C" fn net_client_thread_at() {
    let (x, y) = unsafe { NET_CLIENT_POS };
    spawn_net_client_inner(x, y);
}

fn spawn_net_client_inner(x: i32, y: i32) {
    unsafe {
        write_child_proc_to_disk();

        let mut manifest = Manifest::NONE
            .allow(CapKind::Surface)
            .allow(CapKind::Socket)
            .allow(CapKind::PortIoRange);
        #[cfg(not(feature = "agent_download_unauthorized"))]
        {
            manifest = manifest.allow(CapKind::FileObject);
        }
        #[cfg(not(feature = "spawn_unauthorized"))]
        {
            manifest = manifest.allow(CapKind::ExecHandle);
        }

        let mut requests = alloc::vec::Vec::new();
        requests.push(CapRequest {
            kind: CapKind::Surface,
            object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: SURFACE_WIDTH, height: SURFACE_HEIGHT },
            rights: Rights::MAP,
            label: "net_client_surface",
        });
        requests.push(CapRequest {
            kind: CapKind::Socket,
            object_kind: KernelObjectKind::Socket {
                protocol: SocketProtocol::Tcp,
                local_port: 53000,
                remote_ip: [10, 0, 2, 2],
                remote_port: 80,
            },
            rights: Rights::SEND,
            label: "net_client_socket",
        });
        requests.push(CapRequest {
            kind: CapKind::PortIoRange,
            object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
            rights: Rights::PORT_IO,
            label: "net_client_com1",
        });
        #[cfg(not(feature = "agent_download_unauthorized"))]
        requests.push(CapRequest {
            kind: CapKind::FileObject,
            object_kind: KernelObjectKind::FileObject { inode: 11 },
            rights: Rights::READ.union(Rights::WRITE),
            label: "net_client_file",
        });
        #[cfg(not(feature = "spawn_unauthorized"))]
        requests.push(CapRequest {
            kind: CapKind::ExecHandle,
            object_kind: KernelObjectKind::ExecHandle,
            rights: Rights::EXEC,
            label: "net_client_exec",
        });

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

        #[cfg(not(feature = "agent_download_unauthorized"))]
        let file_cap = 2;
        #[cfg(feature = "agent_download_unauthorized")]
        let file_cap = 99;

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut NetClientInfo;
        core::ptr::write(info_ptr, NetClientInfo {
            surface_cap: 0,
            input_cap,
            socket_cap: 1, // Cap 1 is the granted Socket capability
            ready_token: READY_TOKEN,
            file_cap,
            save_to_disk: 1,
            target_inode: 11,
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
