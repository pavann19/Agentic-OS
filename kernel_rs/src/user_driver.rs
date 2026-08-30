//! Spawns the first REAL user-space driver (Phase 3's "first user-space
//! drivers" item): loads `user_rs/serial_driver`'s actual compiled ELF64
//! binary via `elf.rs`'s real loader, grants it a real `PortIoRange`
//! capability for COM1 through the exact same `driver.rs` mediation path
//! every other capability-gated hardware access in this kernel uses, and
//! enters ring 3 at the binary's own real entry point — not a hardcoded
//! address chosen by the kernel, the address the ELF header itself
//! states.
//!
//! Scope note (also in each driver crate's own module doc): all three
//! `docs/ROADMAP.md` names for this item are now ported — serial (port
//! I/O), framebuffer (MMIO, independently verified), and PS/2 keyboard
//! (the first to use an `InterruptLine` capability from ring 3, via a
//! real, ongoing wait/ack syscall pair, not a one-shot signal). Every
//! ELF is embedded at kernel build time via `include_bytes!`, since
//! there is no filesystem yet (Phase 4) to load it from at runtime;
//! that's a real, honest interim source, not a simulated one — see
//! `elf.rs`'s doc comment.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, ipc, klog_info, pmm, ring3, syscall, thread, vmm};

/// The real, compiled output of `user_rs/serial_driver` -- built
/// separately (its own crate, own target, own linker script; see that
/// crate's Cargo.toml/`.cargo/config.toml`), embedded here as raw bytes.
/// `elf.rs::load()` parses this exactly as it would parse bytes read
/// from a real disk later.
static SERIAL_DRIVER_ELF: &[u8] = include_bytes!(
    "../../user_rs/serial_driver/target/x86_64-unknown-none/release/serial_driver"
);

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

pub fn spawn_serial_driver() {
    thread::spawn(serial_driver_thread);
}

extern "C" fn serial_driver_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, SERIAL_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("USER_DRIVER_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            DRIVER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real capability grant BEFORE this process ever reaches ring 3:
        // a dedicated, throwaway CapabilityTable exists only long enough
        // to mediate the one decision that matters -- opening exactly
        // COM1's 8 ports (0x3F8-0x3FF) in the TSS IOPB. Once
        // grant_port_access returns, the table itself is no longer
        // needed: the CPU enforces the IOPB in hardware from then on,
        // same as driver.rs's module doc states.
        let mut table = CapabilityTable::new();
        let cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, cap) {
            Ok(()) => klog_info!("USER_DRIVER_PORT_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("USER_DRIVER_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!(
            "USER_DRIVER_ELF_ENTER entry=0x{:x} stack=0x{:x}",
            entry,
            DRIVER_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
    }
}

// --- Framebuffer driver ------------------------------------------------
//
// Same pattern as the serial driver above, but proves a different
// capability kind (MmioRegion, not PortIoRange) and closes the loop with
// an INDEPENDENT kernel-side readback rather than trusting the driver's
// own self-report -- see user_rs/framebuffer_driver's module doc for why
// that distinction matters for what this actually proves.

static FRAMEBUFFER_DRIVER_ELF: &[u8] = include_bytes!(
    "../../user_rs/framebuffer_driver/target/x86_64-unknown-none/release/framebuffer_driver"
);

const FB_DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const FB_INFO_VADDR: u64 = 0x0000_0000_0051_0000;
const FB_TEST_MARKER: u32 = 0xAABB_CCDD;

#[repr(C)]
struct FbInfo {
    base_vaddr: u64,
    width: u32,
    height: u32,
    pixels_per_scan_line: u32,
}

struct FbBootParams {
    phys_base: u64,
    size: u64,
    width: u32,
    height: u32,
    pixels_per_scan_line: u32,
}

// Single-core simplification, same pattern/justification as every other
// kernel-wide `static mut` in this codebase (syscall.rs's
// RING3_IPC_TABLE, device_manager.rs's GLOBAL) -- one instance, one
// core, no lock needed.
static mut FB_PARAMS: Option<FbBootParams> = None;

/// Spawns both the real ring-3 framebuffer driver AND a kernel-side
/// verify thread that independently confirms what it wrote. `phys_base`/
/// `size`/`width`/`height`/`pixels_per_scan_line` come from the REAL GOP
/// framebuffer `boot_rs` found at boot (`BootInfo.payload.framebuffer`),
/// not synthetic values.
pub fn spawn_framebuffer_driver(phys_base: u64, size: u64, width: u32, height: u32, pixels_per_scan_line: u32) {
    unsafe {
        FB_PARAMS = Some(FbBootParams { phys_base, size, width, height, pixels_per_scan_line });
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(fb_verify_thread);
    thread::spawn(fb_driver_thread);
}

extern "C" fn fb_driver_thread() {
    unsafe {
        let params = (&*(&raw const FB_PARAMS)).as_ref().unwrap();
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, FRAMEBUFFER_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("USER_DRIVER_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            FB_DRIVER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real MmioRegion capability grant for the actual GOP
        // framebuffer, mediated through the exact same driver.rs path
        // iommu.rs's own comments describe -- map_mmio resolves the
        // capability (Rights::MAP required), then maps every covered
        // page RW+NX+cache-disabled into the DRIVER's address space
        // (`space`, not the kernel's), returning the vaddr it chose.
        let mut table = CapabilityTable::new();
        let cap = driver::create_mmio_capability(&mut table, params.phys_base, params.size, Rights::MAP);
        let fb_vaddr = match driver::map_mmio(&table, cap, space) {
            Ok(v) => v,
            Err(e) => {
                klog_info!("USER_DRIVER_FB_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!(
            "USER_DRIVER_FB_MAPPED phys=0x{:x} size={} -> vaddr=0x{:x}",
            params.phys_base, params.size, fb_vaddr
        );

        // The real, if minimal, boot-time ABI: a dedicated page holding
        // FbInfo, mapped read-only-enough (still writable here since the
        // kernel itself writes it before ring 3 ever runs; nothing
        // ring-3 side needs write access to it) at a fixed vaddr this
        // specific driver knows to read.
        let info_page_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_page_phys) as *mut FbInfo;
        core::ptr::write(
            info_ptr,
            FbInfo {
                base_vaddr: fb_vaddr,
                width: params.width,
                height: params.height,
                pixels_per_scan_line: params.pixels_per_scan_line,
            },
        );
        vmm::map_page_in(
            space,
            FB_INFO_VADDR,
            info_page_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("USER_DRIVER_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, FB_DRIVER_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, FB_DRIVER_STACK_VADDR + 4096);
    }
}

/// Blocks on the real capability-gated syscall 4 the framebuffer driver
/// sends once it's written its test pattern, then reads back the SAME
/// physical framebuffer memory through the KERNEL's own MMIO window
/// (`vmm::map_mmio_page` -- a completely separate mapping from the
/// driver's own, proving this isn't just reading back what the verify
/// thread itself wrote). This independent readback is what makes the
/// proof real: the marker could only be there if the ring-3 driver
/// genuinely wrote it into the genuine physical framebuffer.
extern "C" fn fb_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match ipc::receive(table, cap) {
        Ok(msg) => klog_info!("USER_DRIVER_FB_READY token=0x{:x}", msg.data[0]),
        Err(e) => {
            klog_info!("USER_DRIVER_FB_VERIFY_FAILED to receive ready signal: {:?}", e);
            return;
        }
    }
    unsafe {
        let params = (&*(&raw const FB_PARAMS)).as_ref().unwrap();
        let kernel_vaddr = vmm::map_mmio_page(params.phys_base);
        let observed = core::ptr::read_volatile(kernel_vaddr as *const u32);
        if observed == FB_TEST_MARKER {
            klog_info!("USER_DRIVER_FB_VERIFIED pixel0=0x{:x} (matches expected marker)", observed);
        } else {
            klog_info!("USER_DRIVER_FB_VERIFY_MISMATCH pixel0=0x{:x} expected=0x{:x}", observed, FB_TEST_MARKER);
        }
    }
}

// --- Keyboard driver -----------------------------------------------------
//
// Third real user-space driver: same standalone-ELF pattern, but proves
// a real ring-3 process blocking on an `InterruptLine` capability (via
// syscalls 5/6, syscall.rs) instead of a one-shot MMIO/port grant. See
// user_rs/keyboard_driver's module doc for the honest scope note on why
// this project's automated headless test harness can't itself generate a
// real keystroke to exercise the full path end to end.

static KEYBOARD_DRIVER_ELF: &[u8] = include_bytes!(
    "../../user_rs/keyboard_driver/target/x86_64-unknown-none/release/keyboard_driver"
);

const KBD_DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

pub fn spawn_keyboard_driver() {
    thread::spawn(keyboard_driver_thread);
}

extern "C" fn keyboard_driver_thread() {
    unsafe {
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, KEYBOARD_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("USER_DRIVER_ELF_LOAD_FAILED {:?}", e);
                return;
            }
        };

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            KBD_DRIVER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Real PortIoRange grant for the PS/2 controller's data+status/
        // command ports (0x60, 0x64) -- same one-time IOPB mediation
        // pattern as the serial driver's COM1 grant, just a different
        // real port range.
        let mut port_table = CapabilityTable::new();
        let port_cap = driver::create_port_capability(&mut port_table, 0x60, 5, Rights::PORT_IO);
        match driver::grant_port_access(&port_table, port_cap) {
            Ok(()) => klog_info!("USER_DRIVER_PORT_GRANTED base=0x60 count=5"),
            Err(e) => {
                klog_info!("USER_DRIVER_PORT_GRANT_FAILED {:?}", e);
                return;
            }
        }

        // Real InterruptLine capability for the real, unmasked IRQ1
        // (pic.rs::KEYBOARD_VECTOR / idt.rs::h_keyboard) -- set up here,
        // once, before ring 3 is ever entered; syscalls 5/6 reuse this
        // exact dedicated table on every subsequent wait/ack call.
        syscall::init_kbd_capability();

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!(
            "USER_DRIVER_ELF_ENTER entry=0x{:x} stack=0x{:x}",
            entry,
            KBD_DRIVER_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(entry, KBD_DRIVER_STACK_VADDR + 4096);
    }
}
