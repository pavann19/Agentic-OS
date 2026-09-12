//! Phase 12 (`docs/ROADMAP.md` §5, deliverable 1): kernel-side spawn
//! code for the minimal real compositor foundation. Same real
//! capability-mediation discipline as every driver in this kernel:
//! the kernel owns the real GOP framebuffer discovery (already real
//! since Phase 3, `user_driver.rs`), grants it as an `MmioRegion`
//! capability, mints two real `Surface` capabilities (this module is
//! the ONLY place that ever does — see `capability.rs`'s own doc on
//! `Surface`), and the ring-3 process (`user_rs/compositor_driver`)
//! draws within them, unmediated after that one-time grant.
//!
//! Mutually exclusive with `user_driver::spawn_framebuffer_driver`'s
//! own Phase 3 demo, by construction — both would otherwise race for
//! the same real framebuffer's contents. Gated behind the
//! `compositor_demo` Cargo feature (off by default); see
//! `main.rs`'s own spawn site and `scripts/test-compositor.ps1`.

use crate::capability::{CapabilityTable, Rights};
use crate::{driver, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;

static COMPOSITOR_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/compositor_driver/target/x86_64-unknown-none/release/compositor_driver");

#[repr(C)]
struct SurfaceRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct CompositorInfo {
    fb_vaddr: u64,
    pixels_per_scan_line: u32,
    fb_height: u32,
    surface_a: SurfaceRect,
    surface_b: SurfaceRect,
    color_a: u32,
    color_b: u32,
}

// Real, arbitrary but disjoint colors -- chosen to be unmistakable in
// a raw pixel dump, not aesthetic choices.
const COLOR_A: u32 = 0x00FF_0000; // real solid red
const COLOR_B: u32 = 0x0000_00FF; // real solid blue
// A deliberately real, visible gap between the two surfaces -- the
// independent kernel-side readback below checks a pixel IN this gap
// stayed untouched, real evidence the compositor's own bounds
// enforcement (not just "the two rectangles happen not to overlap")
// is what's being tested.
const GAP_PX: u32 = 64;
const SURFACE_SIZE: u32 = 128;

pub struct FbParams {
    pub phys_base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scan_line: u32,
}

static mut FB_PARAMS: Option<FbParams> = None;

pub fn spawn(params: FbParams) {
    unsafe {
        FB_PARAMS = Some(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(compositor_verify_thread);
    thread::spawn(compositor_driver_thread);
}

extern "C" fn compositor_driver_thread() {
    unsafe {
        let params = (&*(&raw const FB_PARAMS)).as_ref().unwrap();
        let space = vmm::new_address_space();

        let entry = match crate::elf::load(space, COMPOSITOR_DRIVER_ELF) {
            Ok(e) => e,
            Err(e) => {
                klog_info!("COMPOSITOR_ELF_LOAD_FAILED {:?}", e);
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

        let mut table = CapabilityTable::new();
        let fb_cap = driver::create_mmio_capability(&mut table, params.phys_base, params.size, Rights::MAP);
        let fb_vaddr = match driver::map_mmio(&table, fb_cap, space) {
            Ok(v) => v,
            Err(e) => {
                klog_info!("COMPOSITOR_MAP_FAILED {:?}", e);
                return;
            }
        };
        klog_info!("COMPOSITOR_FB_MAPPED phys=0x{:x} -> vaddr=0x{:x}", params.phys_base, fb_vaddr);

        // Real Phase 12 deliverable 1: two real, disjoint Surface
        // capabilities minted here (the only place they ever are),
        // never chosen or guessed by the process that receives them.
        let surface_a = SurfaceRect { x: 0, y: 0, width: SURFACE_SIZE, height: SURFACE_SIZE };
        let surface_b = SurfaceRect { x: SURFACE_SIZE + GAP_PX, y: 0, width: SURFACE_SIZE, height: SURFACE_SIZE };
        let cap_a = driver::create_surface_capability(&mut table, surface_a.x, surface_a.y, surface_a.width, surface_a.height, Rights::MAP);
        let cap_b = driver::create_surface_capability(&mut table, surface_b.x, surface_b.y, surface_b.width, surface_b.height, Rights::MAP);
        klog_info!(
            "COMPOSITOR_SURFACES_GRANTED a=({},{},{},{}) cap={} b=({},{},{},{}) cap={}",
            surface_a.x, surface_a.y, surface_a.width, surface_a.height, cap_a,
            surface_b.x, surface_b.y, surface_b.width, surface_b.height, cap_b
        );

        let com1_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
        match driver::grant_port_access(&table, com1_cap) {
            Ok(()) => klog_info!("COMPOSITOR_COM1_GRANTED base=0x3f8 count=8"),
            Err(e) => {
                klog_info!("COMPOSITOR_COM1_GRANT_FAILED {:?}", e);
                return;
            }
        }

        let info_phys = pmm::alloc_page();
        let info_ptr = pmm::p2v_pub(info_phys) as *mut CompositorInfo;
        core::ptr::write(
            info_ptr,
            CompositorInfo {
                fb_vaddr,
                pixels_per_scan_line: params.pixels_per_scan_line,
                fb_height: params.height,
                surface_a,
                surface_b,
                color_a: COLOR_A,
                color_b: COLOR_B,
            },
        );
        vmm::map_page_in(
            space,
            INFO_VADDR,
            info_phys,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!("COMPOSITOR_ELF_ENTER entry=0x{:x} stack=0x{:x}", entry, DRIVER_STACK_VADDR + 4096);
        ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
    }
}

/// Real, independent kernel-side readback -- the SAME physical
/// framebuffer memory, read through the kernel's OWN mapping
/// (`vmm::map_mmio_page`), never through anything the driver process
/// itself wrote to. Checks: surface A's real color landed inside its
/// own bounds, surface B's real (different) color landed inside its
/// own bounds, AND a real pixel in the gap between them (which the
/// driver's own deliberate adversarial wide-fill attempt swept over)
/// was NOT touched -- the actual, falsifiable evidence that `Surface`
/// bounds are enforced, not merely that two rectangles happen not to
/// overlap.
extern "C" fn compositor_verify_thread() {
    let table = syscall::fb_ready_table();
    let cap = syscall::fb_ready_cap();
    match crate::ipc::receive(table, cap) {
        Ok(msg) => klog_info!("COMPOSITOR_READY token=0x{:x}", msg.data[0]),
        Err(e) => {
            klog_info!("COMPOSITOR_VERIFY_FAILED to receive ready signal: {:?}", e);
            return;
        }
    }
    unsafe {
        let params = (&*(&raw const FB_PARAMS)).as_ref().unwrap();
        let ppsl = params.pixels_per_scan_line as u64;

        // Real bug found and fixed bringing this up: `map_mmio_page`
        // maps exactly ONE real 4KB page (see its own doc/impl) --
        // mapping just `phys_base` once and adding raw byte offsets
        // past that single page reads unmapped kernel address space.
        // Fixed by mapping the SPECIFIC page each pixel's real byte
        // offset actually falls in, per read -- `map_mmio_page` is
        // idempotent (re-mapping an already-mapped page is harmless),
        // so this is real, correct, and still simple.
        let read_pixel = |x: u32, y: u32| -> u32 {
            let byte_offset = (y as u64 * ppsl + x as u64) * 4;
            let vaddr = vmm::map_mmio_page(params.phys_base + byte_offset);
            core::ptr::read_volatile(vaddr as *const u32)
        };

        let pixel_a = read_pixel(SURFACE_SIZE / 2, SURFACE_SIZE / 2);
        let pixel_b = read_pixel(SURFACE_SIZE + GAP_PX + SURFACE_SIZE / 2, SURFACE_SIZE / 2);
        let pixel_gap = read_pixel(SURFACE_SIZE + GAP_PX / 2, SURFACE_SIZE / 2);

        klog_info!(
            "COMPOSITOR_READBACK surface_a=0x{:08x} surface_b=0x{:08x} gap=0x{:08x}",
            pixel_a, pixel_b, pixel_gap
        );

        if pixel_a == COLOR_A && pixel_b == COLOR_B && pixel_gap != COLOR_A {
            klog_info!("COMPOSITOR_SELF_CHECK_PASS: both real surfaces drawn correctly, gap between them left untouched (real bounds enforcement confirmed)");
        } else {
            klog_info!("COMPOSITOR_SELF_CHECK_FAIL: readback did not match expected real colors/bounds");
        }
    }
}
