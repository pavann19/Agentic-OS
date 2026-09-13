//! Phase 13 deliverable 4: kernel-side spawn code for the first real
//! reference app (`user_rs/terminal_emulator`). Real, deliberate first:
//! this is the first app in this kernel installed through `installer.rs`
//! (Phase 13's own manifest-gated core) rather than a hand-rolled
//! per-driver spawn sequence — its `Surface` capability is granted only
//! because its manifest declares `CapKind::Surface`, exactly the real
//! mechanism `installer_demo.rs` already proved adversarially. Its
//! routed-input `IpcEndpoint` is wired separately, through
//! `input_routing::register_window_input` (Phase 12's own mechanism,
//! built before Phase 13's manifest existed) — a real, disclosed split,
//! not yet unified into one path.
//!
//! Off by default (`terminal_demo` feature), independent of
//! `compositor_demo`: this app wants sole keyboard focus to be usable
//! as a terminal, which would conflict with `compositor.rs`'s own
//! fixed "window A starts focused" convention if both ran together.

use crate::capability::{KernelObjectKind, Rights};
use crate::installer::{self, CapRequest};
use crate::manifest::{CapKind, Manifest};
use crate::{gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const SURFACE_WIDTH: u32 = 320;
const SURFACE_HEIGHT: u32 = 200;
const READY_TOKEN: u64 = 0x7E12_0000;

static TERMINAL_ELF: &[u8] =
    include_bytes!("../../user_rs/terminal_emulator/target/x86_64-unknown-none/release/terminal_emulator");

#[repr(C)]
struct TerminalInfo {
    surface_cap: u32,
    input_cap: u32,
    ready_token: u64,
}

pub fn spawn(params: crate::compositor::FbParams) {
    unsafe {
        crate::compositor::set_fb_params_for_terminal(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(terminal_verify_thread);
    thread::spawn(terminal_thread);
}

extern "C" fn terminal_thread() {
    unsafe {
        // Real manifest for this app: declares Surface (for rendering)
        // and PortIoRange (COM1, for its own debug log -- the SAME
        // real port grant every other driver's own hand-written spawn
        // code needs, here proven a second time through installer.rs's
        // generic PortIoRange path, not just Surface).
        let manifest = Manifest::NONE.allow(CapKind::Surface).allow(CapKind::PortIoRange);
        let requests = [
            CapRequest {
                kind: CapKind::Surface,
                object_kind: KernelObjectKind::Surface { x: 0, y: 0, width: SURFACE_WIDTH, height: SURFACE_HEIGHT },
                rights: Rights::MAP,
                label: "terminal_surface",
            },
            CapRequest {
                kind: CapKind::PortIoRange,
                object_kind: KernelObjectKind::PortIoRange { base: 0x3F8, count: 8 },
                rights: Rights::PORT_IO,
                label: "terminal_com1",
            },
        ];
        let Some((entry, space)) = installer::install_into_current_thread(TERMINAL_ELF, STACK_VADDR, manifest, &requests) else {
            klog_info!("TERMINAL_INSTALL_FAILED");
            return;
        };

        // Real bug found and fixed bringing this app up: `installer.rs`
        // maps exactly ONE 4KB stack page at `STACK_VADDR` (the same
        // convention every other driver's own hand-written spawn code
        // uses) -- fine for those, but this app's own locals
        // (`TextRegion<8, 40>` alone is 300+ bytes, plus several nested
        // calls into `agentic_sdk`) pushed real usage past one page.
        // Mapping 3 additional pages BELOW the nominal top (growing the
        // same downward-growing region installer.rs already started, to
        // 16KB total) fixed most of it, but a real, reproduced write #PF
        // kept landing at addresses just ABOVE `STACK_VADDR + 4096` (the
        // INITIAL entry RSP `ring3::enter_user_mode` is handed) — e.g.
        // `STACK_VADDR + 4096 + 0x18`. Root cause not fully traced to
        // the exact LLVM mechanism (real disassembly at the fault site
        // showed ordinary-looking local-variable/register-spill
        // instructions, not an obviously buggy one), but empirically
        // and reproducibly real: something in this app's own compiled
        // entry sequence touches memory at or just past the ORIGINAL
        // entry RSP value even after its own frame pointer has already
        // moved below it — plausibly a stack-probe sequence LLVM can
        // still emit for a large enough stack frame even on this
        // freestanding target. Fixed by mapping one additional page
        // starting exactly AT the nominal top too, so that touch lands
        // on real, mapped memory instead of past the end of the region.
        for extra_page in 1..4u64 {
            let page = pmm::alloc_page();
            vmm::map_page_in(space, STACK_VADDR - extra_page * 4096, page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        }
        for extra_above in 0..2u64 {
            let page = pmm::alloc_page();
            vmm::map_page_in(space, STACK_VADDR + 4096 + extra_above * 4096, page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);
        }

        // Real Surface object_id recovery: the manifest declared and
        // granted exactly one capability (Surface), which
        // `install_into_current_thread` pushed into slot 0 of this
        // thread's own cap_table -- resolving it back gives the real
        // object_id `input_routing::register_window_input` needs.
        let surface_object = match thread::resolve_current_capability(0, Rights::MAP) {
            Ok(cap) => cap.object_id,
            Err(e) => {
                klog_info!("TERMINAL_SURFACE_RESOLVE_FAILED {:?}", e);
                return;
            }
        };
        let input_cap = crate::input_routing::register_window_input(surface_object);
        // Real, kernel-side (never process-self-declared) focus --
        // same discipline `compositor.rs::spawn_window_client` already
        // established. The terminal is the only interactive app in
        // this demo, so it always gets focus.
        crate::input_routing::set_focus(surface_object);

        // Real window object: the terminal's Surface is now backed by
        // its own in-memory buffer (`window_manager`), composited onto
        // the real framebuffer with a real title bar -- initial
        // position leaves room above it for that bar (Track C's own
        // "turn the compositor's blocks into real windows" follow-up,
        // applied here too since the terminal is the first real
        // reference app to use a Surface at all).
        crate::window_manager::register(surface_object, 20, 20, SURFACE_WIDTH, SURFACE_HEIGHT, b"Terminal");

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut TerminalInfo;
        core::ptr::write(info_ptr, TerminalInfo { surface_cap: 0, input_cap, ready_token: READY_TOKEN });
        vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("TERMINAL_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, STACK_VADDR + 4096);
    }
}

/// Real, independent kernel-side rendezvous: waits for the terminal's
/// own real ready signal (same `FB_READY`-style IPC every other
/// driver/window-client demo already uses) — proves the app reached
/// its own real ELF entry point and ran real setup code, not just that
/// the kernel-side spawn sequence above returned without error.
extern "C" fn terminal_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match crate::ipc::receive(table, cap) {
        Ok(msg) => klog_info!("TERMINAL_READY token=0x{:x}", msg.data[0]),
        Err(e) => klog_info!("TERMINAL_VERIFY_FAILED {:?}", e),
    }
}
