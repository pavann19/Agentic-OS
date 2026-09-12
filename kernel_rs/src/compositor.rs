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

use crate::capability::{self, CapabilityTable, Rights};
use crate::{driver, gdt, klog_info, pmm, ring3, syscall, thread, vmm};

const DRIVER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;
const INFO_VADDR: u64 = 0x0000_0000_0051_0000;

static COMPOSITOR_DRIVER_ELF: &[u8] =
    include_bytes!("../../user_rs/compositor_driver/target/x86_64-unknown-none/release/compositor_driver");
static WINDOW_CLIENT_ELF: &[u8] =
    include_bytes!("../../user_rs/window_client_driver/target/x86_64-unknown-none/release/window_client_driver");

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

// Real, disjoint from surface_a/b above (different Y row entirely) --
// the two real, SEPARATE-PROCESS client surfaces (exit criterion 1),
// distinguished from the single-process demo's own surfaces so both
// can be independently verified without interference.
const CLIENT_ROW_Y: u32 = SURFACE_SIZE + 64;
const COLOR_C: u32 = 0x0000_FF00; // real solid green
const COLOR_D: u32 = 0x00FF_FF00; // real solid yellow
const FOREIGN_CAP_GUESS: u32 = 99; // never granted to either client -- the real adversarial probe

#[repr(C)]
struct WindowClientInfo {
    label: u8,
    surface_cap: u32,
    color: u32,
    foreign_cap_guess: u32,
    // Phase 12 exit criterion 4: this window's own real IpcEndpoint
    // CapId for routed keyboard input (`input_routing::
    // register_window_input`) -- resolved against this SAME process's
    // own cap_table, same structural guarantee `surface_cap` already
    // has (a table-local index, never a global handle another process
    // could guess into).
    input_cap: u32,
}

pub struct FbParams {
    pub phys_base: u64,
    pub size: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scan_line: u32,
}

static mut FB_PARAMS: Option<FbParams> = None;

/// Phase 12 exit criterion 3: a reserved, non-PCI synthetic bus/device/
/// function identity for the compositor's own driver thread -- there is
/// no real PCI device behind it, but `device_manager.rs`/`supervisor.rs`
/// only ever key on this triple, never on any other field of a real
/// `PciDevice`, so a fixed sentinel works exactly like a real bdf would.
/// Bus 0xFE is never reached by `pci::enumerate`'s real scan in any QEMU
/// topology this project boots (a handful of devices on bus 0, nothing
/// past a couple of bridges) -- chosen to be unmistakably synthetic
/// rather than colliding with a real future device.
pub const SYNTHETIC_BUS: u8 = 0xFE;
pub const SYNTHETIC_DEVICE: u8 = 0;
pub const SYNTHETIC_FUNCTION: u8 = 0;

pub fn spawn(params: FbParams) {
    unsafe {
        FB_PARAMS = Some(params);
    }
    syscall::init_fb_ready_ipc();
    thread::spawn(compositor_verify_thread);
    thread::spawn(compositor_driver_thread);
    thread::spawn(window_client_a_thread);
    thread::spawn(window_client_b_thread);
}

/// Phase 9.5a's real respawn entry point, reused for the compositor
/// (registered via `supervisor::register` in `main.rs`): re-invokes the
/// SAME thread entry point `spawn` used the first time, reading the SAME
/// `FB_PARAMS` (still valid across a restart -- the framebuffer's
/// identity doesn't change) -- a respawned compositor runs identical
/// code, not improvised recovery, matching `ahci::respawn`'s own
/// discipline.
pub fn respawn() {
    thread::spawn(compositor_driver_thread);
}

extern "C" fn window_client_a_thread() {
    unsafe { spawn_window_client(b'A', 0, CLIENT_ROW_Y, COLOR_C) };
}
extern "C" fn window_client_b_thread() {
    unsafe { spawn_window_client(b'B', SURFACE_SIZE + GAP_PX, CLIENT_ROW_Y, COLOR_D) };
}

/// Real Phase 12 exit criterion 1 setup: a NEW address space, a NEW,
/// SEPARATE `CapabilityTable` (never shared with `compositor_driver`
/// OR the other client), granted exactly ONE real `Surface`
/// capability and nothing that reaches the framebuffer directly — no
/// `MmioRegion` grant at all. Drawing happens only through
/// `SYS_SURFACE_FILL` (syscall 11), which the kernel mediates against
/// THIS process's own table.
unsafe fn spawn_window_client(label: u8, x: u32, y: u32, color: u32) {
    let space = vmm::new_address_space();

    let entry = match crate::elf::load(space, WINDOW_CLIENT_ELF) {
        Ok(e) => e,
        Err(e) => {
            klog_info!("WINDOW_CLIENT_ELF_LOAD_FAILED {:?}", e);
            return;
        }
    };

    let stack_page = pmm::alloc_page();
    vmm::map_page_in(space, DRIVER_STACK_VADDR, stack_page, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

    // Real, direct grant into THIS thread's own cap_table (the one
    // `thread::resolve_current_capability`/syscall 11 actually reads)
    // -- NOT a throwaway local `CapabilityTable` (see
    // `thread::grant_current_capability`'s own doc for the real bug
    // that distinction fixes).
    let surface_object = capability::create_object(capability::KernelObjectKind::Surface { x, y, width: SURFACE_SIZE, height: SURFACE_SIZE });
    let surface_cap = thread::grant_current_capability(surface_object, Rights::MAP);
    klog_info!("WINDOW_CLIENT_SURFACE_GRANTED label={} rect=({},{},{},{}) cap={}", label as char, x, y, SURFACE_SIZE, SURFACE_SIZE, surface_cap);

    // Phase 12 exit criterion 4: register this window for routed input
    // BEFORE deciding focus below -- `set_focus` looks up this exact
    // surface_object, so the registry entry must already exist.
    let input_cap = crate::input_routing::register_window_input(surface_object);
    // Real, kernel-side (never process-self-declared) initial focus:
    // whichever window is labeled the primary one ('A') starts
    // focused, a fixed, disclosed convention -- there is no window
    // manager or click-to-focus yet (this module's own doc). The
    // window process itself has no way to call this -- it isn't
    // exposed as a syscall at all in this increment.
    if label == b'A' {
        crate::input_routing::set_focus(surface_object);
    }

    let mut table = CapabilityTable::new();
    let com1_cap = driver::create_port_capability(&mut table, 0x3F8, 8, Rights::PORT_IO);
    if driver::grant_port_access(&table, com1_cap).is_err() {
        klog_info!("WINDOW_CLIENT_COM1_GRANT_FAILED label={}", label as char);
        return;
    }

    let info_phys = pmm::alloc_page();
    let info_ptr = pmm::p2v_pub(info_phys) as *mut WindowClientInfo;
    core::ptr::write(info_ptr, WindowClientInfo { label, surface_cap, color, foreign_cap_guess: FOREIGN_CAP_GUESS, input_cap });
    vmm::map_page_in(space, INFO_VADDR, info_phys, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE);

    let kernel_stack_top = thread::current_kernel_stack_top();
    gdt::set_kernel_stack(kernel_stack_top);
    syscall::set_kernel_stack(kernel_stack_top);
    syscall::init();

    vmm::switch_address_space(space);
    thread::set_current_address_space(space);
    klog_info!("WINDOW_CLIENT_ELF_ENTER label={} entry=0x{:x} stack=0x{:x}", label as char, entry, DRIVER_STACK_VADDR + 4096);
    ring3::enter_user_mode(entry, DRIVER_STACK_VADDR + 4096);
}

/// Real syscall-11 handler body (kept here, not `syscall.rs`, since
/// it needs `FB_PARAMS` and the pixel-write helper this module
/// already owns): resolves `cap_id` against the CALLING thread's OWN
/// `cap_table` (never a shared or global one), requires it to be a
/// real `Surface`, then fills EXACTLY that surface's own real bounds
/// — the calling process supplies a `CapId` and a color, nothing
/// else; it never receives a framebuffer pointer, so it has no way to
/// write anywhere else even if it wanted to.
pub fn syscall_fill_surface(cap_id: capability::CapId, color: u32) -> u64 {
    let cap = match thread::resolve_current_capability(cap_id, Rights::MAP) {
        Ok(c) => c,
        Err(_) => {
            klog_info!("SYSCALL_SURFACE_FILL_DENIED cap={}", cap_id);
            return u64::MAX;
        }
    };
    let (x, y, width, height) = match capability::object_kind(cap.object_id) {
        Some(capability::KernelObjectKind::Surface { x, y, width, height }) => (x, y, width, height),
        _ => {
            klog_info!("SYSCALL_SURFACE_FILL_WRONG_KIND cap={}", cap_id);
            return u64::MAX;
        }
    };
    unsafe {
        let params = match (&*(&raw const FB_PARAMS)).as_ref() {
            Some(p) => p,
            None => return u64::MAX,
        };
        let ppsl = params.pixels_per_scan_line as u64;
        for py in y..y + height {
            for px in x..x + width {
                let byte_offset = (py as u64 * ppsl + px as u64) * 4;
                let vaddr = vmm::map_mmio_page(params.phys_base + byte_offset);
                core::ptr::write_volatile(vaddr as *mut u32, color);
            }
        }
    }
    klog_info!("SYSCALL_SURFACE_FILL_OK cap={} rect=({},{},{},{}) color=0x{:08x}", cap_id, x, y, width, height, color);
    0
}

/// Real request struct a caller writes into its OWN mapped memory
/// before invoking `SYS_SURFACE_DRAW_TEXT` -- the syscall ABI here
/// only carries two plain integer arguments (`a0`/`a1`), so a request
/// with more fields than that (position, both colors, and a text
/// pointer/length) is passed by reference, the same shape every driver
/// crate's own `INFO_VADDR`-mapped info struct already uses to hand the
/// kernel more than two words of setup data.
#[repr(C)]
struct SurfaceTextRequest {
    x: u32,
    y: u32,
    fg: u32,
    bg: u32,
    text_vaddr: u64,
    text_len: u32,
}

/// Real, disclosed bound on a single draw call's text -- generous for
/// one line of a terminal/editor/file-manager row (Phase 13's own
/// planned reference apps), not a general unbounded string API.
const MAX_DRAW_TEXT_LEN: u32 = 256;

/// Phase 12 deliverable 4: SYS_SURFACE_DRAW_TEXT -- `a0` = the CALLER's
/// own `CapId` for a `Surface`, `a1` = the vaddr of a `SurfaceTextRequest`
/// in the CALLER's own mapped memory. Same real per-process isolation
/// as `syscall_fill_surface`: the Surface capability is resolved
/// against the CALLING thread's own `cap_table` only, and every drawn
/// pixel is bounds-checked against THAT surface's own real rectangle
/// (`text::draw_text`'s own per-pixel clip) -- a request naming
/// coordinates or a text length that would reach outside the caller's
/// own surface is truncated, never drawn into another process's
/// region, structurally, not by convention.
pub fn syscall_draw_text(cap_id: capability::CapId, request_vaddr: u64) -> u64 {
    let cap = match thread::resolve_current_capability(cap_id, Rights::MAP) {
        Ok(c) => c,
        Err(_) => {
            klog_info!("SYSCALL_SURFACE_DRAW_TEXT_DENIED cap={}", cap_id);
            return u64::MAX;
        }
    };
    let (sx, sy, swidth, sheight) = match capability::object_kind(cap.object_id) {
        Some(capability::KernelObjectKind::Surface { x, y, width, height }) => (x, y, width, height),
        _ => {
            klog_info!("SYSCALL_SURFACE_DRAW_TEXT_WRONG_KIND cap={}", cap_id);
            return u64::MAX;
        }
    };
    unsafe {
        let pml4 = vmm::current_cr3();
        let req_size = core::mem::size_of::<SurfaceTextRequest>() as u64;
        if !vmm::validate_user_buffer_readable(pml4, request_vaddr, req_size) {
            klog_info!("SYSCALL_SURFACE_DRAW_TEXT_BAD_REQUEST_PTR cap={}", cap_id);
            return u64::MAX;
        }
        let mut req_bytes = [0u8; core::mem::size_of::<SurfaceTextRequest>()];
        vmm::read_user_bytes(pml4, request_vaddr, &mut req_bytes);
        let req: SurfaceTextRequest = core::ptr::read_unaligned(req_bytes.as_ptr() as *const SurfaceTextRequest);

        let text_len = req.text_len.min(MAX_DRAW_TEXT_LEN) as usize;
        if text_len == 0 || !vmm::validate_user_buffer_readable(pml4, req.text_vaddr, text_len as u64) {
            klog_info!("SYSCALL_SURFACE_DRAW_TEXT_BAD_TEXT_PTR cap={}", cap_id);
            return u64::MAX;
        }
        let mut text_buf = [0u8; MAX_DRAW_TEXT_LEN as usize];
        vmm::read_user_bytes(pml4, req.text_vaddr, &mut text_buf[..text_len]);

        let params = match (&*(&raw const FB_PARAMS)).as_ref() {
            Some(p) => p,
            None => return u64::MAX,
        };
        crate::text::draw_text(
            params.phys_base,
            params.pixels_per_scan_line,
            sx + req.x,
            sy + req.y,
            &text_buf[..text_len],
            req.fg,
            req.bg,
            sx,
            sy,
            swidth,
            sheight,
        );
    }
    klog_info!("SYSCALL_SURFACE_DRAW_TEXT_OK cap={} rect=({},{},{},{})", cap_id, sx, sy, swidth, sheight);
    0
}

extern "C" fn compositor_driver_thread() {
    unsafe {
        // Phase 9.5a: record THIS thread as the current owner of the
        // compositor's synthetic device identity -- the same discipline
        // `ahci_driver_thread` uses -- so a later real fault on this
        // exact thread traces back to the compositor via
        // `supervisor::on_process_killed`, which has nothing but
        // `thread::current_id()` to work with.
        crate::supervisor::mark_thread_owner(SYNTHETIC_BUS, SYNTHETIC_DEVICE, SYNTHETIC_FUNCTION);
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
    // Real, sequential rendezvous drain -- one real message per real
    // process that signals readiness (the single-process
    // `compositor_driver` demo, plus the two, real, SEPARATE window
    // client processes below). `ipc::receive`'s own rendezvous
    // semantics (see its module doc) make the ARRIVAL order
    // irrelevant here: this thread only needs all three real
    // processes to have finished before reading pixels, not to know
    // which one finished first.
    for _ in 0..3 {
        match crate::ipc::receive(table, cap) {
            Ok(msg) => klog_info!("COMPOSITOR_READY token=0x{:x}", msg.data[0]),
            Err(e) => {
                klog_info!("COMPOSITOR_VERIFY_FAILED to receive ready signal: {:?}", e);
                return;
            }
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

        // Real Phase 12 exit criterion 1 verification: the SAME real
        // independent readback, now against the two SEPARATE-PROCESS
        // client surfaces. Each color landing correctly, drawn only
        // through a kernel-mediated syscall neither client could have
        // reached the other's region through even by mistake (no
        // shared memory, no shared pointer, no way to name the
        // other's `CapId`), is the actual, falsifiable "one process
        // cannot reach another's window buffer" proof.
        let pixel_c = read_pixel(SURFACE_SIZE / 2, CLIENT_ROW_Y + SURFACE_SIZE / 2);
        let pixel_d = read_pixel(SURFACE_SIZE + GAP_PX + SURFACE_SIZE / 2, CLIENT_ROW_Y + SURFACE_SIZE / 2);
        let pixel_client_gap = read_pixel(SURFACE_SIZE + GAP_PX / 2, CLIENT_ROW_Y + SURFACE_SIZE / 2);

        klog_info!(
            "COMPOSITOR_MULTIPROC_READBACK client_a=0x{:08x} client_b=0x{:08x} gap=0x{:08x}",
            pixel_c, pixel_d, pixel_client_gap
        );

        if pixel_c == COLOR_C && pixel_d == COLOR_D && pixel_client_gap != COLOR_C && pixel_client_gap != COLOR_D {
            klog_info!("COMPOSITOR_MULTIPROC_SELF_CHECK_PASS: two SEPARATE real processes each drew only their own real Surface, mediated entirely through syscall_fill_surface -- neither reached the other's or the gap between them");
        } else {
            klog_info!("COMPOSITOR_MULTIPROC_SELF_CHECK_FAIL: multi-process readback did not match expected real colors/bounds");
        }

        // Phase 12 deliverable 4: real, independent readback that PSF1
        // text actually landed -- both window clients drew a real
        // two-line block via SYS_SURFACE_DRAW_TEXT at their own
        // surface's top-left corner (local (0,0)) before this. Rather
        // than assume a specific glyph's exact bitmap (font-dependent,
        // never inspected here), this scans a small region covering
        // that block and counts pixels matching the real foreground
        // color used (0x00FFFFFF): a real glyph render produces SOME
        // (the strokes) but not ALL (the gaps between/around them)
        // matching pixels -- a uniform result either way (0 or every
        // pixel) would mean nothing was actually drawn, not a real
        // bitmap pattern.
        const TEXT_FG: u32 = 0x00FF_FFFF;
        let count_fg = |base_x: u32, base_y: u32| -> (u32, u32) {
            let mut matches = 0u32;
            let mut total = 0u32;
            for dy in 0..32u32 {
                for dx in 0..16u32 {
                    if read_pixel(base_x + dx, base_y + dy) == TEXT_FG {
                        matches += 1;
                    }
                    total += 1;
                }
            }
            (matches, total)
        };
        let (text_a_fg, text_a_total) = count_fg(0, CLIENT_ROW_Y);
        let (text_b_fg, text_b_total) = count_fg(SURFACE_SIZE + GAP_PX, CLIENT_ROW_Y);
        klog_info!(
            "COMPOSITOR_TEXT_READBACK client_a_fg={}/{} client_b_fg={}/{}",
            text_a_fg, text_a_total, text_b_fg, text_b_total
        );
        let real_glyph_pattern = |fg: u32, total: u32| fg > 0 && fg < total;
        if real_glyph_pattern(text_a_fg, text_a_total) && real_glyph_pattern(text_b_fg, text_b_total) {
            klog_info!("COMPOSITOR_TEXT_SELF_CHECK_PASS: both windows' real PSF1 text produced a genuine glyph pattern (neither blank nor solid), independently read back");
        } else {
            klog_info!("COMPOSITOR_TEXT_SELF_CHECK_FAIL: text readback did not show a real glyph pattern");
        }
    }
}
