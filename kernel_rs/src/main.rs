//! Agentic OS kernel — Rust port, Phase 0.
//!
//! Boot path so far: validate BootInfo, bring up serial/klog, initialize
//! the PMM (`pmm.rs`, ported from `kernel/memory.c`), then replace the
//! bootloader's temporary identity-mapped page tables with real,
//! permission-correct higher-half ones (`vmm.rs` — a redesign, not a port;
//! see its module doc for why). GDT/IDT/heap/timer/interrupts are not
//! ported yet — see `PHASE0_PROGRESS.md`.
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

extern crate alloc;

pub mod acpi;
pub mod authority; // research track -- real wiring, see authority.rs module doc
#[cfg(feature = "research_authority_hw_demo")]
pub mod authority_hw_fault_demo; // research track -- live-device fault-after-revocation demo, off by default (see Cargo.toml)
pub mod ahci;
pub mod agent;
pub mod apic;
pub mod audit;
pub mod bootinfo;
pub mod capability;
pub mod compositor; // Phase 12 -- minimal real compositor foundation, see its own module doc
pub mod compositor_metrics; // Phase 5.0 -- lightweight compositor performance telemetry
pub mod critical;
pub mod device_manager;
pub mod damage; // Phase 5.2 -- damage region tracking system with non-allocating rect lists
pub mod driver;
pub mod e1000;
pub mod elf;
pub mod fault_isolation_demo;
pub mod file_manager; // Phase 13 deliverable 4 -- third real reference app spawn code, see its own module doc
pub mod file_service; // real block/file-I/O-for-apps path (IPC-mediated), see its own module doc
pub mod mouse_driver; // real GUI mouse support (PS/2, IRQ12), see its own module doc
pub mod init;
pub mod events;
pub mod interrupt_forward;
pub mod introspect;
pub mod input_queue; // Phase 5.1 -- lock-free input event queue decoupling input from rendering
pub mod frame_scheduler; // Phase 5.7 -- frame pacing and deadline scheduling
pub mod input_routing; // Phase 12 exit criterion 4 -- minimal real keyboard input routing, see its own module doc
pub mod installer; // Phase 13 deliverable 2 -- real manifest-gated app install core, see its own module doc
pub mod installer_demo; // Phase 13 -- real adversarial installer demo against a genuine ELF app, off by default (see Cargo.toml)
pub mod ipc;
pub mod manifest; // Phase 13 deliverable 1 -- per-app capability manifest, see its own module doc
pub mod manifest_demo; // Phase 13 -- real adversarial manifest-enforcement demo, off by default (see Cargo.toml)
#[cfg(any(
    feature = "fault_test_null_deref",
    feature = "fault_test_rodata_write",
    feature = "fault_test_nx_exec",
    feature = "fault_test_double_fault"
))]
pub mod fault_injection;
pub mod gdt;
pub mod heap;
pub mod idt;
pub mod iommu;
pub mod klog;
pub mod object_store;
pub mod nvme;
pub mod netstack; // Phase 10 -- real network-stack process, see its own module doc
pub mod net_service; // Phase 13 -- network service for ring-3 apps
pub mod net_client_app; // Phase 13 deliverable 4 -- fourth real reference app
pub mod sample_app_demo; // Phase 13 exit criterion 2 -- third-party SDK app demo
pub mod package; // Phase 13 deliverable 5 -- package store & reproducible updates
pub mod launcher; // Phase 13 deliverable 2 -- real app launcher
pub mod pci;
pub mod pic;
pub mod pmm;
pub mod policy;
pub mod renderer; // Phase 5.11 -- compositor renderer abstraction (CpuRenderer / GpuRenderer)
pub mod ring3;
pub mod serial;
pub mod serial_input;
pub mod service_manager;
pub mod shell;
pub mod smp;
pub mod smp_race_soak; // Phase 9 deliverable 3's real cross-core race evidence, see its own module doc
pub mod socket_demo; // Phase 10 -- real adversarial Socket capability revocation demo, off by default (see Cargo.toml)
pub mod supervisor; // Phase 9.5a -- real crash-to-restart supervision, see its own module doc
pub mod syscall;
pub mod terminal; // Phase 13 deliverable 4 -- first real reference app spawn code, see its own module doc
pub mod text_editor; // Phase 13 deliverable 4 -- second real reference app spawn code, see its own module doc
pub mod text; // Phase 12 deliverable 4 -- minimal real PSF1 text rendering, see its own module doc
pub mod thread;
pub mod tools;
pub mod user_driver;
pub mod usb_xhci; // Phase 11 -- real xHCI (USB) host controller discovery, see its own module doc
pub mod virtio_blk;
pub mod virtio_gpu; // Phase 5.13 -- VirtIO-GPU hardware graphics path and probe
pub mod virtio_net;
pub mod vmm;
pub mod vsync; // Phase 5.12 -- VSync and presentation synchronization
pub mod window_manager; // real window objects (movable, own backing buffer, title bar), see its own module doc

use bootinfo::BootInfo;
use vmm::KernelSegment;

// Defined by linker.ld — mark each section's virtual/physical boundaries so
// the kernel can describe its own layout to vmm::init() without re-parsing
// its own ELF at runtime. These are addresses, not values — `&__text_start`
// gives the linked address; the symbol itself has no meaningful "content".
extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
    static __bss_start: u8;
    static __bss_end: u8;
}

const KERNEL_VIRTUAL_BASE: u64 = vmm::KERNEL_VIRTUAL_BASE;

fn vaddr_of(sym: &u8) -> u64 {
    sym as *const u8 as u64
}

/// Real, disclosed GUI chrome: a real light-gray desktop background
/// plus a real, fixed top menu bar (white, with a real "Agentic OS"
/// label) -- the classic-Mac-System look this desktop now goes for.
/// Real, disclosed scope: this is fixed, static chrome (the menu bar
/// never opens real dropdown menus yet) -- a real, honest visual
/// improvement, not a functional Finder-style menu system. Colors and
/// height are `window_manager`'s own real constants (not a second,
/// driftable copy here) -- that module's own `redraw_rect` repaints
/// this exact same chrome, per-pixel, whenever the cursor moves over
/// bare desktop.
#[allow(dead_code)]
unsafe fn draw_desktop_chrome(fb_phys_base: u64, ppsl: u32, width: u32, height: u32) {
    let size = (height as u64 * ppsl as u64) * 4;
    vmm::map_framebuffer_range(fb_phys_base, size);
    window_manager::init_desktop_chrome(fb_phys_base, ppsl, width, height);
}

#[no_mangle]
pub extern "sysv64" fn kernel_main(boot_info: *const BootInfo) -> ! {
    klog::init();

    let info = match unsafe { bootinfo::validate(boot_info) } {
        Ok(info) => info,
        Err(reason) => klog::panic(reason),
    };

    klog_info!("KERNEL_ENTER");
    klog_info!("BootInfo version={} size={}", info.version, info.size);

    // GDT/IDT come up before PMM/VMM deliberately: a fault during the
    // risky page-table-rebuild-and-CR3-switch work below needs a real
    // handler to be diagnosable at all. This ordering was decided
    // mid-session after a CR3 switch produced a silent, undiagnosable
    // hang with no exception handling in place yet — see PHASE0_PROGRESS.md.
    gdt::init_for_cpu(0); // BSP is always cpu_index 0 (Phase 9 deliverable 2 -- per-CPU GDT/TSS)
    idt::init();

    klog_info!("PMM_INIT_START");
    unsafe { pmm::init(info) };
    klog_info!("PMM_INIT_DONE");

    klog_info!("VMM_INIT_START");
    let segments = unsafe {
        [
            KernelSegment {
                vaddr: vaddr_of(&__text_start),
                paddr: vaddr_of(&__text_start) - KERNEL_VIRTUAL_BASE,
                len: vaddr_of(&__text_end) - vaddr_of(&__text_start),
                writable: false,
                executable: true,
            },
            KernelSegment {
                vaddr: vaddr_of(&__rodata_start),
                paddr: vaddr_of(&__rodata_start) - KERNEL_VIRTUAL_BASE,
                len: vaddr_of(&__rodata_end) - vaddr_of(&__rodata_start),
                writable: false,
                executable: false,
            },
            KernelSegment {
                vaddr: vaddr_of(&__data_start),
                paddr: vaddr_of(&__data_start) - KERNEL_VIRTUAL_BASE,
                len: vaddr_of(&__data_end) - vaddr_of(&__data_start),
                writable: true,
                executable: false,
            },
            KernelSegment {
                vaddr: vaddr_of(&__bss_start),
                paddr: vaddr_of(&__bss_start) - KERNEL_VIRTUAL_BASE,
                len: vaddr_of(&__bss_end) - vaddr_of(&__bss_start),
                writable: true,
                executable: false,
            },
        ]
    };
    let current_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, rsp", out(reg) current_rsp, options(nomem, nostack, preserves_flags));
    }
    unsafe { vmm::init(info, &segments, current_rsp) };
    klog_info!("VMM_INIT_DONE");
    // Real root-cause fix for the reported input/redraw lag (see
    // `vmm::enable_pat_write_combining`'s own doc): must run before
    // ANY real framebuffer page is ever mapped, so it goes here, right
    // after the page tables it reprograms exist, and well before
    // compositor_demo/terminal_demo touch the framebuffer.
    unsafe { vmm::enable_pat_write_combining() };

    // Real bug this session found: `info` was validated against the
    // BOOTSTRAP identity mapping boot_rs set up, back before this line —
    // once vmm::init() switches to the kernel's own production tables
    // (which do NOT identity-map arbitrary physical memory, only kernel
    // segments + the direct-map window + heap + MMIO), the OLD `info`
    // reference silently points at now-unmapped memory. Every use of it
    // between here and the actual crash happened to not touch it again
    // until Phase 3's ACPI code did — manifested as a page fault reading
    // BootInfo's own rsdp field. Fixed by re-deriving a reference through
    // the direct-map window (the same translation pmm.rs uses for every
    // other post-switch physical access), which stays valid for the rest
    // of the kernel's lifetime.
    let info: &BootInfo = unsafe { &*(pmm::p2v_pub(boot_info as u64) as *const BootInfo) };

    #[cfg(any(
        feature = "fault_test_null_deref",
        feature = "fault_test_rodata_write",
        feature = "fault_test_nx_exec",
        feature = "fault_test_double_fault"
    ))]
    fault_injection::run();

    klog_info!("HEAP_INIT_START");
    heap::init();
    klog_info!("HEAP_INIT_DONE");

    // Real smoke test, not just "it linked": alloc::vec::Vec exercises the
    // global allocator through the same path any future kernel code would.
    {
        use alloc::vec::Vec;
        let mut v: Vec<u32> = Vec::new();
        for i in 0..256u32 {
            v.push(i * i);
        }
        let sum: u64 = v.iter().map(|&x| x as u64).sum();
        klog_info!("HEAP_SMOKE_TEST_OK sum={} len={}", sum, v.len());
    }

    pic::remap_and_mask_all(0x20, 0x28);
    // Phase 3: PS/2 keyboard needs its real IRQ (1) unmasked -- every
    // OTHER legacy PIC line stays masked, still fully correct for
    // pic.rs's own "APIC timer only" reasoning above (this clears
    // exactly one bit).
    pic::unmask_irq(1);
    // Real GUI mouse support: IRQ12 (PS/2 mouse, the 8042's auxiliary
    // port) lives on the SLAVE PIC -- a slave-PIC line also needs its
    // own cascade line (IRQ2) unmasked on the MASTER PIC, or the
    // slave's own interrupts never reach the CPU at all regardless of
    // IRQ12 itself being unmasked (standard dual-8259 cascade wiring).
    pic::unmask_irq(2);
    pic::unmask_irq(12);
    klog_info!("TIMER_INIT_START");
    apic::init();
    // Phase 9: configure the BSP's own SYSCALL/SYSRET MSRs unconditionally,
    // here, rather than leaving it to whichever driver-setup thread
    // happens to call syscall::init() first -- see smp.rs::ap_entry's own
    // doc comment on the real work-stealing-migration bug this closes
    // (a thread can migrate to a core between calling syscall::init()
    // and its own next `syscall`); every AP does the equivalent in its
    // own ap_entry, so this makes the invariant ("every core has
    // SYSCALL/SYSRET configured before any ring-3 thread can possibly
    // run there") true for the BSP too, explicitly, not just by luck of
    // call-site ordering.
    crate::syscall::init();
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    klog_info!("TIMER_INIT_DONE");

    // Phase 1: kernel threads + preemptive scheduling. kernel_main itself
    // becomes "thread 0" (its execution continues below exactly as
    // before -- this call just makes it visible to the scheduler so
    // h_timer's schedule() has a valid thread to save/restore starting on
    // the very next tick). Real bug found and fixed under WHPX (the fast,
    // real interrupt timing exposed it; TCG's slower/serialized timing
    // never did): this call used to sit much later in kernel_main, AFTER
    // every Phase 3+ driver's own thread::spawn (AHCI, virtio, NVMe, xHCI,
    // e1000, init, the driver processes, fault_isolation_demo, the agent
    // demos, the shell -- 14 real threads' worth by the time it ran).
    // Called that late, `NEXT_TID` was already 14, so kernel_main's own
    // placeholder zero-length "not ours to own" stack (see this
    // function's own doc comment) got registered under a normal-looking
    // thread id and, on the very next tick, silently entered the SAME
    // round-robin run queue as every real thread -- nothing here or in
    // schedule_locked ever distinguished it as special. Once genuinely
    // selected as `next` in that queue (an ordinary event, not an edge
    // case), schedule_locked's own unconditional per-switch TSS.RSP0
    // update (thread.rs) programmed the CPU's real ring3->ring0
    // kernel-stack pointer to this thread's placeholder address --
    // corrupting the ONE piece of hardware state every OTHER thread's own
    // next ring-3 interrupt depends on being correct. Fixed by restoring
    // the function's own documented invariant: called here, BEFORE this
    // kernel's very first thread::spawn (virtio_blk below), so kernel_main
    // truly is thread 0 -- NEXT_TID is 0 at this call, matching what the
    // comment above already claimed but the call site's real position no
    // longer did.
    thread::init_as_current_thread();

    klog_info!("Rust kernel slice: PMM+VMM+heap+timer+deferred-IRQ-queue live, higher-half.");
    klog_info!("NOTE: graphics/keyboard not yet ported (Phase 0 core items complete).");

    // Phase 3: real PCIe enumeration against whatever this machine
    // actually has, not a synthetic/mocked device list.
    klog_info!("PCI_ENUMERATE_START");
    let pci_devices = pci::enumerate();
    pci::log_all(&pci_devices);
    klog_info!("PCI_ENUMERATE_DONE count={}", pci_devices.len());

    // Phase 4: real virtio-blk block device driver, spawned (if the
    // device is present) as a real ELF-loaded ring-3 process -- same
    // capability-gated pattern as every Phase 3 driver, extended to a
    // real DMA-safe buffer + IOMMU domain assignment.
    virtio_blk::spawn_if_present(&pci_devices);
    virtio_net::spawn_if_present(&pci_devices);
    virtio_gpu::probe_and_init(&pci_devices);
    ahci::spawn_if_present(&pci_devices);
    nvme::spawn_if_present(&pci_devices);
    // Phase 11 (docs/ROADMAP.md Sec5, deliverable 2): real xHCI (USB)
    // host controller discovery. Real class-code matching, not an
    // exact vendor/device ID -- absent on a machine/QEMU config with
    // no xHCI controller, this is a real, harmless no-op (see
    // usb_xhci.rs's own module doc for why class-code matching is the
    // spec-correct approach here).
    usb_xhci::spawn_if_present(&pci_devices);
    // Phase 10: netstack.rs REPLACES e1000.rs's own device ownership
    // when the network_stack feature is on -- both would otherwise
    // race for the same PCI device's BAR/DMA (see netstack.rs's own
    // module doc). Default boot (feature off) is completely unaffected.
    #[cfg(feature = "network_stack")]
    netstack::spawn_if_present(&pci_devices);
    #[cfg(not(feature = "network_stack"))]
    e1000::spawn_if_present(&pci_devices);

    // Phase 3: device manager -- discovery, driver binding, lifecycle,
    // restart-on-crash. Consumes the real device list above; the
    // classification, binding decisions, and lifecycle transitions below
    // are all real, though nothing yet spawns an actual user-space
    // driver process (the NEXT unstarted Phase 3 item) -- bind_all()
    // records a real decision with no process behind it yet.
    klog_info!("DEVMGR_INIT_START");
    let mut devmgr = device_manager::DeviceManager::new();
    devmgr.discover(&pci_devices);
    devmgr.bind_all();
    // Phase 9.5a: the real SATA/AHCI controller (00:1f.2) -- previously
    // brought up to Running and then given a DELIBERATELY SIMULATED
    // crash+recovery here, to prove the restart-on-crash state machine
    // against a real device entry before any real supervisor existed to
    // drive it from an actual fault (see device_manager.rs's own
    // module-doc honesty note, and docs/PROGRESS.md's Phase 3 section
    // for that original evidence). That simulated call is REMOVED now
    // that `supervisor.rs` drives `report_crash` from a REAL
    // `PROCESS_KILLED` event on this exact device -- keeping both would
    // double-transition the same device's state machine and make real
    // and simulated evidence indistinguishable in the log, exactly the
    // ambiguity Phase 9.5a's own exit criteria require avoiding.
    devmgr.mark_running(0, 0x1f, 2);
    devmgr.log_summary();
    device_manager::install_global(devmgr);
    klog_info!("DEVMGR_INIT_DONE");
    supervisor::register(0, 0x1f, 2, ahci::respawn);

    // Phase 3: init and a service manager as the first user-space
    // processes. spawn_init() starts service_manager_thread (kernel-side
    // for now -- see service_manager.rs's honesty note) and a REAL ring-3
    // `init` process; the two rendezvous through a real capability-gated
    // syscall (SVC_START), not a hardcoded boot-order assumption.
    klog_info!("INIT_SPAWN_START");
    init::spawn_init();
    klog_info!("INIT_SPAWN_DONE");

    // Phase 3: first REAL user-space driver -- a genuine compiled ELF64
    // binary (user_rs/serial_driver), loaded by elf.rs's real loader,
    // granted a real PortIoRange capability for COM1, entered at its own
    // real entry point. See user_driver.rs's module doc for scope.
    klog_info!("USER_DRIVER_SPAWN_START");
    user_driver::spawn_serial_driver();
    klog_info!("USER_DRIVER_SPAWN_DONE");

    // Phase 3: second real user-space driver -- the REAL GOP framebuffer
    // boot_rs found at boot, granted to a genuine ELF-loaded ring-3
    // process as a real MmioRegion capability. See user_driver.rs's
    // module doc for why the verification (an independent kernel-side
    // readback, not the driver's own self-report) is what makes this
    // real rather than just another log line.
    unsafe {
        // Real bug found bringing this up: info.payload.framebuffer is a
        // PHYSICAL pointer (UEFI pool memory boot_rs allocated it in),
        // same class of bug as the info: &BootInfo dangling-reference
        // fix in this same function -- dereferencing it directly here
        // faulted (vector 14, user=false, present=false) the instant
        // this code ran, since it's not identity-mapped under the
        // kernel's OWN production tables. Fixed the same way: translate
        // through pmm::p2v_pub before dereferencing.
        let fb_ptr = pmm::p2v_pub(info.payload.framebuffer as u64) as *const bootinfo::Framebuffer;
        let fb = &*fb_ptr;
        klog_info!(
            "FRAMEBUFFER_FOUND base=0x{:x} size={} width={} height={} pixels_per_scan_line={}",
            fb.base_address as u64, fb.buffer_size, fb.width, fb.height, fb.pixels_per_scan_line
        );
        klog_info!("USER_DRIVER_FB_SPAWN_START");
        // Phase 12: `compositor.rs`'s own minimal real compositor
        // foundation REPLACES this Phase 3 demo when the
        // `compositor_demo` feature is on -- both would otherwise race
        // for the same real framebuffer's contents (same mutual-
        // exclusion discipline `netstack.rs`/`e1000.rs` already use).
        // Default boot (feature off) is completely unaffected.
        #[cfg(feature = "compositor_demo")]
        {
            // Phase 12 deliverable 4: real PSF1 font, already loaded by
            // boot_rs since Phase 0 (`bootinfo::BootInfoPayload::font`)
            // but never used by kernel_rs until now.
            text::init(info.payload.font);
            // Real, disclosed fix: UEFI/TianoCore's own boot splash is
            // still sitting in the real GOP framebuffer at this point --
            // nothing before this ever cleared it, so any pixel outside
            // whatever a surface/window draws stays boot-splash content
            // forever. One real, whole-screen clear before anything else
            // draws.
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            // Real, evidence-backed latency fix (Phase 13 / latency plan
            // step 3): bulk-map every page in the framebuffer's physical
            // span once, with Write-Combining, BEFORE any window content
            // is rendered. After this call map_framebuffer_page hits its
            // LAST_FB_PAGE_PADDR cache on every subsequent call (no
            // page-table walk), and window_manager::present_partial can
            // use a single copy_nonoverlapping per scanline instead of
            // per-pixel map+store. Logged in QEMU serial: see
            // VMM_FB_RANGE_MAPPED in the boot log for verification.
            vmm::map_framebuffer_range(fb.base_address as u64, fb.buffer_size as u64);
            mouse_driver::spawn();
            serial_input::spawn();
            compositor::spawn(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
            // Phase 12 exit criterion 3: wire the compositor into the
            // SAME real crash-to-restart mechanism Phase 9.5a proved on
            // AHCI/netstack -- `device_manager.rs` requires a
            // pre-existing `ManagedDevice` entry for `report_crash` to
            // do anything, so `register_synthetic` gives the compositor
            // one under its reserved, non-PCI bdf identity.
            device_manager::with_global(|dm| {
                dm.register_synthetic(
                    compositor::SYNTHETIC_BUS,
                    compositor::SYNTHETIC_DEVICE,
                    compositor::SYNTHETIC_FUNCTION,
                    device_manager::DriverKind::Compositor,
                )
            });
            supervisor::register(compositor::SYNTHETIC_BUS, compositor::SYNTHETIC_DEVICE, compositor::SYNTHETIC_FUNCTION, compositor::respawn);
        }
        // Phase 13 deliverable 4: the terminal_emulator reference app --
        // mutually exclusive with compositor_demo (it wants sole
        // keyboard focus, which would conflict with compositor.rs's own
        // fixed "window A starts focused" convention).
        #[cfg(feature = "terminal_demo")]
        {
            text::init(info.payload.font);
            // Real, disclosed fix for the reported "boot splash still
            // visible behind TERMINAL_EMULATOR_READY" bug -- see the
            // identical clear_screen call in the compositor_demo branch
            // above for the full explanation.
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            mouse_driver::spawn();
            serial_input::spawn();
            terminal::spawn(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
        }
        // Phase 13 deliverable 4: the text_editor reference app -- same
        // sole-keyboard-focus exclusion as terminal_demo above.
        #[cfg(feature = "text_editor_demo")]
        {
            text::init(info.payload.font);
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            mouse_driver::spawn();
            serial_input::spawn();
            text_editor::spawn(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
        }
        // Phase 13 deliverable 4: the file_manager reference app -- same
        // sole-keyboard-focus exclusion as the other demos above. Needs
        // a real virtio-blk device actually attached to show real
        // content (see scripts/test-file-manager.ps1); otherwise it
        // reports "NO FILE SERVER REGISTERED" honestly rather than
        // showing anything fake.
        #[cfg(feature = "file_manager_demo")]
        {
            text::init(info.payload.font);
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            mouse_driver::spawn();
            serial_input::spawn();
            file_manager::spawn(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
        }
        // Phase 13 deliverable 4: the net_client reference app
        #[cfg(feature = "net_client_demo")]
        {
            text::init(info.payload.font);
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            mouse_driver::spawn();
            serial_input::spawn();
            net_client_app::spawn(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
        }
        // Phase 13 exit criterion 2: third-party SDK app demo
        #[cfg(feature = "sample_app_demo")]
        {
            text::init(info.payload.font);
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            mouse_driver::spawn();
            serial_input::spawn();
            sample_app_demo::spawn(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
        }
        // Phase 13 exit criterion 3: all four reference apps running concurrently
        #[cfg(feature = "phase13_all")]
        {
            text::init(info.payload.font);
            draw_desktop_chrome(fb.base_address as u64, fb.pixels_per_scan_line, fb.width, fb.height);
            mouse_driver::spawn();
            serial_input::spawn();
            launcher::launch_all_apps(compositor::FbParams {
                phys_base: fb.base_address as u64,
                size: fb.buffer_size,
                width: fb.width,
                height: fb.height,
                pixels_per_scan_line: fb.pixels_per_scan_line,
            });
        }
        #[cfg(not(any(
            feature = "compositor_demo",
            feature = "terminal_demo",
            feature = "text_editor_demo",
            feature = "file_manager_demo",
            feature = "net_client_demo",
            feature = "sample_app_demo",
            feature = "phase13_all"
        )))]
        user_driver::spawn_framebuffer_driver(
            fb.base_address as u64,
            fb.buffer_size,
            fb.width,
            fb.height,
            fb.pixels_per_scan_line,
        );
        klog_info!("USER_DRIVER_FB_SPAWN_DONE");
    }

    // Phase 3: third real user-space driver -- PS/2 keyboard, the first
    // to use a real InterruptLine capability from ring 3 (see
    // user_driver.rs's module doc for the honest scope note on headless
    // automated verification).
    klog_info!("USER_DRIVER_KBD_SPAWN_START");
    user_driver::spawn_keyboard_driver();
    klog_info!("USER_DRIVER_KBD_SPAWN_DONE");

    // Phase 1's long-deferred exit criterion, finally closed (see
    // fault_isolation_demo.rs and idt.rs::recover_or_halt): a real
    // ring-3 process deliberately faults here, and the log below THIS
    // point continuing to show other threads finishing their own work is
    // the actual proof the fault killed only this one process, not the
    // whole kernel.
    klog_info!("FAULT_ISOLATION_DEMO_SPAWN_START");
    fault_isolation_demo::spawn_fault_isolation_demo();
    klog_info!("FAULT_ISOLATION_DEMO_SPAWN_DONE");

    // Phase 5's agent process model + structured introspection API --
    // see agent.rs's module doc for the full mapping of this one call to
    // three of Phase 5's four exit criteria.
    klog_info!("AGENT_DEMO_SPAWN_START");
    agent::spawn_agent_demo();
    klog_info!("AGENT_DEMO_SPAWN_DONE");

    // Phase 7's text shell -- see shell.rs's module doc. Only active when
    // no graphical reference app owns sole interactive focus on COM1.
    #[cfg(not(any(
        feature = "compositor_demo",
        feature = "terminal_demo",
        feature = "text_editor_demo",
        feature = "file_manager_demo",
        feature = "net_client_demo",
        feature = "sample_app_demo",
        feature = "phase13_all"
    )))]
    shell::spawn_shell();

    // Phase 3: ACPI table discovery -- the real RSDP boot_rs found via the
    // UEFI configuration table, walked to find DMAR (the IOMMU's register
    // base) for the item below.
    klog_info!("ACPI_INIT_START rsdp=0x{:x}", info.payload.rsdp as u64);
    let xsdt = acpi::init(info.payload.rsdp as u64);
    match xsdt {
        Some(xsdt_phys) => {
            klog_info!("ACPI_INIT_DONE xsdt=0x{:x}", xsdt_phys);

            // Phase 9's first deliverable (docs/ROADMAP.md §5): real
            // MADT CPU enumeration. Logged only, no AP bring-up yet --
            // INIT-SIPI-SIPI is real, correctness-critical, hard-to-
            // debug-if-wrong work that deliberately lands as its own
            // increment once this enumeration step is itself verified
            // against real multi-CPU QEMU boot evidence.
            let mut cpus = [kernel_common::madt::CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 32];
            let cpu_count = acpi::find_cpus(xsdt_phys, &mut cpus);
            if cpu_count == 0 {
                klog_info!("ACPI_MADT_NOT_FOUND (no CPU topology exposed by firmware)");
            } else {
                let mut enabled_count = 0u32;
                for c in &cpus[..cpu_count] {
                    klog_info!(
                        "ACPI_MADT_CPU apic_id={} processor_uid={} enabled={}",
                        c.apic_id, c.processor_uid, c.enabled
                    );
                    if c.enabled {
                        enabled_count += 1;
                    }
                }
                klog_info!("ACPI_MADT_DONE total={} enabled={}", cpu_count, enabled_count);

                // Phase 9 deliverable 1's real bring-up step -- see
                // smp.rs's own module doc for the full design (identity
                // -map trick, real bounded timeouts throughout). Every
                // core that comes up now stays a real, independently
                // scheduled participant (deliverable 3) rather than
                // halting.
                let brought_up = smp::bring_up_all(&cpus[..cpu_count]);

                if brought_up >= 2 {
                    // Phase 9 deliverable 3's real, live evidence: a
                    // thread explicitly placed on ANOTHER core's own
                    // run queue, picked up via a real reschedule IPI
                    // rather than that core's next periodic tick.
                    klog_info!("SMP_PINNED_DEMO_SPAWN target_cpu_index=1");
                    thread::spawn_pinned_to_cpu(smp::pinned_demo_thread, vmm::kernel_pml4_phys(), 1);

                    // Phase 9 deliverable 4's real, live evidence
                    // (`docs/ROADMAP.md` §5 — "a mapping torn down on
                    // one core is provably unusable on another core
                    // within a bounded time"): map a real scratch page
                    // into the shared kernel PML4 (reachable from every
                    // online core, since they all share it), then tear
                    // it down via `vmm::unmap_page_shootdown` instead
                    // of the plain, local-only `vmm::unmap_page`. Real
                    // evidence: `smp::shootdown_tlb`'s own
                    // `TLB_SHOOTDOWN_PASS` log line only fires once
                    // EVERY other online core's own interrupt handler
                    // has actually executed a real `invlpg` and
                    // acknowledged -- not just "the IPI was sent".
                    unsafe {
                        let scratch_phys = pmm::alloc_page();
                        const SHOOTDOWN_DEMO_VADDR: u64 = 0x0000_0000_0090_0000;
                        vmm::map_page_in(vmm::kernel_pml4_phys(), SHOOTDOWN_DEMO_VADDR, scratch_phys, vmm::PAGE_WRITABLE);
                        klog_info!("SMP_TLB_SHOOTDOWN_DEMO_START vaddr=0x{:x}", SHOOTDOWN_DEMO_VADDR);
                        vmm::unmap_page_shootdown(vmm::kernel_pml4_phys(), SHOOTDOWN_DEMO_VADDR);
                    }
                }
                if brought_up >= 3 {
                    // Deliverable 3's exit criterion: a deliberate
                    // cross-core race against a real shared kernel
                    // structure, caught rather than silently
                    // corrupting state -- see smp_race_soak.rs's own
                    // module doc. Run as its OWN spawned coordinator
                    // thread (NOT called inline here) so its real,
                    // necessarily-bounded spin-waits never block the
                    // rest of boot -- and racers are pinned to cpu 1/2
                    // specifically, leaving the BSP (cpu 0) free to
                    // keep running the normal boot sequence
                    // concurrently, exactly the real concurrency this
                    // deliverable is meant to demonstrate.
                    thread::spawn(smp_race_soak::run_as_thread);
                } else {
                    klog_info!("SMP_RACE_SOAK_SKIPPED (fewer than 3 real cores online -- needs 2 real racer cores distinct from the BSP)");
                }
            }

            match acpi::find_table(xsdt_phys, b"DMAR") {
                Some(dmar_phys) => {
                    klog_info!("ACPI_DMAR_FOUND phys=0x{:x}", dmar_phys);
                    klog_info!("IOMMU_INIT_START");
                    if iommu::init(dmar_phys) {
                        klog_info!("IOMMU_INIT_DONE");
                        // Phase 6's containment exit criterion: real,
                        // ongoing fault monitoring, not a one-shot test
                        // hook -- see iommu.rs's own doc.
                        iommu::spawn_fault_monitor();
                        // Real domain assignment for a real device this
                        // session's own PCI enumeration found: the SATA/
                        // AHCI controller (00:1f.2). Grants it exactly one
                        // 4KB physical page -- everything else on the
                        // system stays unreachable to this device by
                        // construction (empty page tables for any address
                        // outside this range), not by kernel-side policy.
                        let dma_buffer_phys = unsafe { pmm::alloc_page() };
                        let _domain = iommu::assign_device(0, 0x1f, 2, &[(dma_buffer_phys, 4096)]);
                        klog_info!(
                            "IOMMU_DOMAIN_ASSIGNED device=00:1f.2 mapped_phys=0x{:x} len=4096",
                            dma_buffer_phys
                        );

                        // Research track (docs/RESEARCH_TRACK.md,
                        // docs/NOVEL_CONCEPTS.md §1): the real hardware
                        // revocation self-check, PLUS the live-device
                        // fault-after-revocation escalation below. Both
                        // gated behind the `research_authority_hw_demo`
                        // feature, OFF by default -- found, by a real
                        // test-suite failure (test-keyboard.ps1's fixed
                        // boot-wait timing broke once this block's real,
                        // deliberate bounded wait for an
                        // expected-to-fail command added genuine
                        // wall-clock delay to every boot), that research-
                        // track runtime cost must not touch the default
                        // boot path at all -- same discipline
                        // fault_injection.rs/ring3.rs's own demo already
                        // established for exactly this reason. Every
                        // step below reads back the REAL hardware-facing
                        // IOMMU context-table bytes
                        // (iommu::context_entry_present) -- not kernel
                        // bookkeeping about that state -- so this is
                        // real evidence, not a simulation of the claim.
                        #[cfg(feature = "research_authority_hw_demo")]
                        {
                            const TEST_BUS: u8 = 0;
                            const TEST_DEV: u8 = 0x1d;
                            const TEST_FUNC: u8 = 7;
                            let test_phys = unsafe { pmm::alloc_page() };

                            let (reachable_before, hw_before) = authority::cross_check(TEST_BUS, TEST_DEV, TEST_FUNC, test_phys);
                            klog_info!("AUTHORITY_HW_SELFCHECK_START graph_reachable={} hw_present={} (both must be false before any grant)", reachable_before, hw_before);

                            let granted = authority::grant_device(TEST_BUS, TEST_DEV, TEST_FUNC, test_phys, 4096);
                            let (reachable_after_grant, hw_after_grant) = authority::cross_check(TEST_BUS, TEST_DEV, TEST_FUNC, test_phys);
                            klog_info!(
                                "AUTHORITY_HW_SELFCHECK_GRANT domain={} graph_reachable={} hw_present={} (both must be true)",
                                granted.map(|d| d.0).unwrap_or(0), reachable_after_grant, hw_after_grant
                            );

                            let revoked = authority::revoke_device(TEST_BUS, TEST_DEV, TEST_FUNC, test_phys);
                            let (reachable_after_revoke, hw_after_revoke) = authority::cross_check(TEST_BUS, TEST_DEV, TEST_FUNC, test_phys);
                            klog_info!(
                                "AUTHORITY_HW_SELFCHECK_REVOKE revoked_ok={} graph_reachable={} hw_present={} (both must be false again)",
                                revoked, reachable_after_revoke, hw_after_revoke
                            );

                            if !reachable_before && !hw_before
                                && reachable_after_grant && hw_after_grant
                                && !reachable_after_revoke && !hw_after_revoke
                            {
                                klog_info!("AUTHORITY_HW_SELFCHECK_PASS: real IOMMU context-table state tracked the authority graph exactly, grant and revoke, both verified against real hardware-facing bytes");
                            } else {
                                klog_info!("AUTHORITY_HW_SELFCHECK_FAIL: software and hardware state disagreed at some point -- see the three lines above for exactly where");
                            }
                        }

                        // Research track escalation (docs/RESEARCH_TRACK.md):
                        // a real, LIVE PCI device (the same AHCI
                        // controller 00:1f.2 this kernel's own driver
                        // speaks to) issuing a real DMA-backed command
                        // after its grant is revoked -- see
                        // authority_hw_fault_demo.rs's own module doc
                        // for why this runs kernel-side rather than
                        // coordinating with the live ring-3 ahci_driver,
                        // and why the real ahci_driver's own later,
                        // independent self-check is unaffected by it.
                        // Same feature gate as the block above -- see
                        // its comment for why this must stay off the
                        // default boot path.
                        #[cfg(feature = "research_authority_hw_demo")]
                        authority_hw_fault_demo::run(0, 0x1f, 2);

                        // Sections 1 (CPU-side completion), 2 (hardware-
                        // bound certificate + real corruption test), and
                        // 3 (envelope discovered from REAL captured
                        // IOMMU fault addresses) -- same feature gate,
                        // same reason: real, deliberate bounded waits
                        // for several expected-to-fault commands add
                        // real wall-clock delay unsuitable for a normal
                        // boot. See docs/RESEARCH_TRACK.md.
                        #[cfg(feature = "research_authority_hw_demo")]
                        authority_hw_fault_demo::run_cpu_side_demo();
                        #[cfg(feature = "research_authority_hw_demo")]
                        authority_hw_fault_demo::run_certificate_corruption_test(0, 0x1f, 2);
                        #[cfg(feature = "research_authority_hw_demo")]
                        authority_hw_fault_demo::run_envelope_discovery(0, 0x1f, 2);
                    } else {
                        klog_info!("IOMMU_INIT_FAILED");
                    }
                }
                None => klog_info!("ACPI_DMAR_NOT_FOUND (no IOMMU exposed by firmware)"),
            }
        }
        None => klog_info!("ACPI_INIT_FAILED"),
    }

    // Drains events::pop() in normal (non-interrupt) context — proves the
    // producer (h_timer, interrupt context)/consumer (here) path works
    // end to end, not just that the counter increments.
    let mut consumed = 0u32;
    while consumed < 3 {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
        while let Some(events::Event::Tick(n)) = events::pop() {
            klog_info!("DEFERRED_EVENT_CONSUMED tick={}", n);
            consumed += 1;
        }
    }
    klog_info!("TIMER_TICKS_OBSERVED count={}", apic::tick_count());

    // thread::init_as_current_thread() now runs much earlier (right after
    // TIMER_INIT_DONE, before any real thread exists) -- see its own call
    // site's doc comment for the real bug this fixes.
    thread::spawn(demo_thread_a);
    thread::spawn(demo_thread_b);
    klog_info!("THREADS_SPAWNED count=2");

    // Phase 1: per-process address spaces. Real proof of isolation, not
    // just "it compiles": two independent address spaces, each with a
    // page mapped at the SAME user virtual address but backed by a
    // DIFFERENT physical page holding different content. A thread bound
    // to each address space reads that shared virtual address and logs
    // what it finds — if isolation is real, they see different values
    // despite using the identical pointer.
    unsafe {
        let space_a = vmm::new_address_space();
        let page_a = pmm::alloc_page();
        *(pmm::p2v_pub(page_a) as *mut u64) = 0xAAAA_AAAA_AAAA_AAAA;
        vmm::map_page_in(space_a, USER_TEST_VADDR, page_a, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE);

        let space_b = vmm::new_address_space();
        let page_b = pmm::alloc_page();
        *(pmm::p2v_pub(page_b) as *mut u64) = 0xBBBB_BBBB_BBBB_BBBB;
        vmm::map_page_in(space_b, USER_TEST_VADDR, page_b, vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE);

        ADDRESS_SPACE_A.store(space_a, core::sync::atomic::Ordering::SeqCst);
        ADDRESS_SPACE_B.store(space_b, core::sync::atomic::Ordering::SeqCst);
    }
    thread::spawn_in(demo_process_a, ADDRESS_SPACE_A.load(core::sync::atomic::Ordering::SeqCst));
    thread::spawn_in(demo_process_b, ADDRESS_SPACE_B.load(core::sync::atomic::Ordering::SeqCst));
    klog_info!("ADDRESS_SPACES_CREATED count=2");

    #[cfg(feature = "demo_ring3")]
    thread::spawn(demo_ring3_thread);

    #[cfg(feature = "socket_revoke_demo")]
    socket_demo::start();

    #[cfg(feature = "manifest_demo")]
    manifest_demo::start();

    #[cfg(feature = "installer_demo")]
    installer_demo::start();

    #[cfg(any(feature = "app_update_demo", feature = "phase13_all"))]
    package::run_app_update_demo();

    // Phase 2: capability substrate. Real proof of every exit criterion
    // from docs/ROADMAP.md's Phase 2 section, not just "it compiles":
    // grant, attenuated derive (both the success case AND the rejected
    // over-broad request), real cross-thread IPC data transfer gated by
    // capability, a denied access from insufficient rights, and immediate
    // revocation that invalidates an already-derived capability.
    unsafe {
        SENDER_TABLE = Some(capability::CapabilityTable::new());
        RECEIVER_TABLE = Some(capability::CapabilityTable::new());
        let sender_table = (&mut *&raw mut SENDER_TABLE).as_mut().unwrap();

        // Full-rights grant, then attenuate down to what each side
        // actually needs — sender gets SEND-only, receiver gets
        // RECEIVE-only, both derived from the SAME underlying object.
        let root_cap = ipc::create_endpoint(
            sender_table,
            capability::Rights::SEND
                .union(capability::Rights::RECEIVE)
                .union(capability::Rights::GRANT)
                .union(capability::Rights::REVOKE),
        );

        // Attenuation, success case: SEND-only is a real subset of what
        // root_cap holds.
        let send_cap = sender_table
            .derive_self(root_cap, capability::Rights::SEND)
            .expect("SEND-only derive must succeed — it's a real subset");
        ENDPOINT_OBJECT_ID.store(
            sender_table.resolve(send_cap, capability::Rights::SEND).unwrap().object_id,
            core::sync::atomic::Ordering::SeqCst,
        );

        // Attenuation, REJECTED case. `send_cap` alone can't demonstrate
        // this correctly (derive() itself requires the SOURCE to hold
        // GRANT just to be usable as a derivation source at all — send_cap
        // has no GRANT, so trying from it fails at that earlier check with
        // InsufficientRights, not the attenuation check this is meant to
        // exercise; found by testing, not anticipated). A real
        // over-broad-request test needs a source that HAS GRANT but
        // genuinely LACKS the right being asked for: derive an
        // intermediate SEND|GRANT capability (a real subset of root_cap,
        // so this derive itself succeeds), then ask it for RECEIVE, which
        // it does not hold.
        let send_and_grant_cap = sender_table
            .derive_self(root_cap, capability::Rights::SEND.union(capability::Rights::GRANT))
            .expect("SEND|GRANT derive must succeed — real subset of root_cap");
        match sender_table.derive_self(send_and_grant_cap, capability::Rights::RECEIVE) {
            Err(capability::CapError::AttenuationViolation) => {
                klog_info!("ATTENUATION_VIOLATION_REJECTED_OK (asked RECEIVE from a SEND|GRANT cap)");
            }
            other => klog_info!("ATTENUATION_VIOLATION_TEST_UNEXPECTED result={:?}", other.is_ok()),
        }

        let receiver_table = (&mut *&raw mut RECEIVER_TABLE).as_mut().unwrap();
        let recv_cap = sender_table
            .derive(root_cap, capability::Rights::RECEIVE, receiver_table)
            .expect("RECEIVE-only derive must succeed");

        SENDER_SEND_CAP.store(send_cap, core::sync::atomic::Ordering::SeqCst);
        RECEIVER_RECV_CAP.store(recv_cap, core::sync::atomic::Ordering::SeqCst);
        ROOT_CAP.store(root_cap, core::sync::atomic::Ordering::SeqCst);

        // Denied-access proof: the SENDER's table holds send_cap (SEND
        // only) — attempting to use it for RECEIVE must fail, logged as a
        // real Denied audit record, not just an assumption.
        match sender_table.resolve(send_cap, capability::Rights::RECEIVE) {
            Err(capability::CapError::InsufficientRights) => {
                klog_info!("DENIED_ACCESS_REJECTED_OK (SEND-only cap used for RECEIVE)");
            }
            other => klog_info!("DENIED_ACCESS_TEST_UNEXPECTED result={:?}", other.is_ok()),
        }
    }
    klog_info!("CAPABILITIES_GRANTED_AND_DERIVED");

    // Phase 4's object store exit criterion, demonstrated directly: "A
    // process without a capability to a file cannot discover that the
    // file exists." Two SEPARATE, otherwise-empty capability tables --
    // one gets a real FileObject capability for the real ext2 file this
    // session's virtio_blk_driver created (inode 11, kernel_common::
    // ext2::FILE_INODE); the other never does. The proof isn't a
    // separate "permission denied" message -- it's that the SAME cap_id
    // in the table that never held it produces the EXACT SAME
    // NoSuchCapability error a bare made-up index would, because there
    // is no separate existence check to fail differently.
    {
        let mut holder_table = capability::CapabilityTable::new();
        let stranger_table = capability::CapabilityTable::new();

        let file_cap = object_store::create_file_capability(
            &mut holder_table,
            11, // kernel_common::ext2::FILE_INODE -- the real file on disk
            capability::Rights::MAP,
        );

        match object_store::resolve_to_inode(&holder_table, file_cap, capability::Rights::MAP) {
            Ok(inode) => klog_info!("OBJSTORE_RESOLVED_OK inode={} (holder table)", inode),
            Err(e) => klog_info!("OBJSTORE_RESOLVE_UNEXPECTED_FAILURE {:?}", e),
        }

        match object_store::resolve_to_inode(&stranger_table, file_cap, capability::Rights::MAP) {
            Err(object_store::ObjectStoreError::Cap(capability::CapError::NoSuchCapability)) => {
                klog_info!("OBJSTORE_DISCOVERY_DENIED_OK (stranger table: NoSuchCapability, indistinguishable from nonexistent)");
            }
            other => klog_info!("OBJSTORE_DISCOVERY_TEST_UNEXPECTED result={:?}", other.is_ok()),
        }
    }

    thread::spawn(demo_ipc_sender);
    thread::spawn(demo_ipc_receiver);

    thread::spawn(demo_interrupt_forward_thread);

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// Real proof of preemptive multithreading, not just "it compiled": two
/// threads each looping and logging their own ID + iteration count.
/// Interleaved output (A/B/A/B, not all of A then all of B) is the actual
/// evidence that h_timer's schedule() call is really swapping between
/// them on timer ticks, not just running whichever happened to be current.
extern "C" fn demo_thread_a() {
    for i in 0..5u32 {
        klog_info!("THREAD_A tick={}", i);
        for _ in 0..80_000_000u64 {
            unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
        }
    }
    klog_info!("THREAD_A_DONE");
}

// extern "C" fn() thread entry points take no arguments, so the address
// each demo process needs to read is passed via these statics instead —
// set once before spawning, read once at thread start. A real process
// abstraction (Phase 1's remaining "process lifecycle" item) would carry
// this per-thread instead; this is the minimal plumbing for THIS proof.
static ADDRESS_SPACE_A: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static ADDRESS_SPACE_B: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const USER_TEST_VADDR: u64 = 0x0000_0000_0040_0000;

extern "C" fn demo_process_a() {
    let value = unsafe { *(USER_TEST_VADDR as *const u64) };
    klog_info!("PROCESS_A read 0x{:x} at 0x{:x}", value, USER_TEST_VADDR);
}

extern "C" fn demo_process_b() {
    let value = unsafe { *(USER_TEST_VADDR as *const u64) };
    klog_info!("PROCESS_B read 0x{:x} at 0x{:x}", value, USER_TEST_VADDR);
}

// Phase 2: capability + IPC demo state. Same "pass via statics" pattern
// as the address-space demo above, same reasoning — extern "C" fn()
// thread entries take no arguments.
static mut SENDER_TABLE: Option<capability::CapabilityTable> = None;
static mut RECEIVER_TABLE: Option<capability::CapabilityTable> = None;
static SENDER_SEND_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static RECEIVER_RECV_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static ROOT_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static ENDPOINT_OBJECT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

extern "C" fn demo_ipc_sender() {
    use core::sync::atomic::Ordering;
    let table = unsafe { (*&raw const SENDER_TABLE).as_ref().unwrap() };
    let cap = SENDER_SEND_CAP.load(Ordering::SeqCst);
    let mut msg = ipc::Message::default();
    msg.data[0] = 0xC0FF_EE00_DEAD_BEEF;
    klog_info!("IPC_SENDER sending 0x{:x}", msg.data[0]);
    match ipc::send(table, cap, msg) {
        Ok(()) => klog_info!("IPC_SENDER send confirmed delivered"),
        Err(e) => klog_info!("IPC_SENDER send FAILED {:?}", e),
    }

    // Revocation proof: revoke the underlying endpoint object, then try
    // to send again on the SAME capability that worked a moment ago —
    // must now be rejected, proving revocation takes effect immediately
    // even though the capability's own table slot is untouched.
    let object_id = ENDPOINT_OBJECT_ID.load(Ordering::SeqCst);
    capability::revoke(object_id, false);
    match ipc::send(table, cap, msg) {
        Err(ipc::IpcError::Cap(capability::CapError::Revoked)) => {
            klog_info!("REVOCATION_REJECTED_OK (same cap_id, post-revoke send denied)");
        }
        other => klog_info!("REVOCATION_TEST_UNEXPECTED result={:?}", matches!(other, Ok(()))),
    }
}

extern "C" fn demo_ipc_receiver() {
    use core::sync::atomic::Ordering;
    let table = unsafe { (*&raw const RECEIVER_TABLE).as_ref().unwrap() };
    let cap = RECEIVER_RECV_CAP.load(Ordering::SeqCst);
    match ipc::receive(table, cap) {
        Ok(msg) => klog_info!("IPC_RECEIVER got 0x{:x}", msg.data[0]),
        Err(e) => klog_info!("IPC_RECEIVER receive FAILED {:?}", e),
    }
}

extern "C" fn demo_interrupt_forward_thread() {
    interrupt_forward::register(apic::TIMER_VECTOR);
    klog_info!("INTERRUPT_FORWARD registered vector=0x{:x}, waiting", apic::TIMER_VECTOR);
    interrupt_forward::wait_for_interrupt(apic::TIMER_VECTOR);
    klog_info!("INTERRUPT_FORWARD received vector=0x{:x}", apic::TIMER_VECTOR);
    interrupt_forward::acknowledge(apic::TIMER_VECTOR);
    klog_info!("INTERRUPT_FORWARD acknowledged");
    audit::dump_all();
    klog_info!("AUDIT_LOG_DUMP_DONE count={}", audit::len());
}

/// Real proof of ring 3, not just "it compiles": user code executing
/// `hlt` (0xF4), a CPL0-only instruction. If this faults with #GP, CPL
/// really was 3 -- kernel code running the identical instruction never
/// faults. See ring3.rs's module doc.
///
/// MUST run as a spawned thread, not inline in kernel_main -- a real bug
/// this session found: kernel_main still runs on its ORIGINAL boot-time
/// stack (a low-address identity mapping from vmm::init(), never migrated
/// to a heap allocation), which lives in the canonical-LOW half and is
/// therefore NOT included in a fresh address space's copied upper half
/// (vmm::new_address_space() only copies indices 256-511). Calling
/// switch_address_space() directly from kernel_main's context unmapped
/// its own currently-in-use stack out from under it, immediately
/// double-faulting. A spawned thread's stack is heap-allocated (already
/// in the upper canonical half), so it survives the switch correctly --
/// confirmed by demo_process_a/b (also spawned threads) switching address
/// spaces via schedule() without incident, while this exact code inline
/// in kernel_main double-faulted.
#[cfg(feature = "demo_ring3")]
extern "C" fn demo_ring3_thread() {
    const USER_CODE_VADDR: u64 = 0x0000_0000_0060_0000;
    const USER_STACK_VADDR: u64 = 0x0000_0000_0070_0000;

    unsafe {
        let space = vmm::new_address_space();

        let code_page = pmm::alloc_page();
        let code_bytes = pmm::p2v_pub(code_page);
        // Phase 2's syscall-surface proof, chained before the Phase 1
        // logging syscall: syscall 2 sends 0xCAFE through a REAL
        // capability-gated IPC endpoint (syscall.rs::init_ring3_ipc_demo,
        // set up below) entirely from ring 3 — enforcement isn't bypassed
        // for syscalls, this goes through the identical
        // CapabilityTable::resolve every other caller does. Then syscall 1
        // (Phase 1's original proof) logs 0x1234. The trailing hlt is the
        // same CPL0-only-instruction proof as before: if SYSRET correctly
        // returned to ring 3 both times, this still faults with #GP
        // exactly as it always has.
        let program: [u8; 25] = [
            0xBF, 0xFE, 0xCA, 0x00, 0x00, // mov edi, 0xCAFE
            0xB8, 0x02, 0x00, 0x00, 0x00, // mov eax, 2
            0x0F, 0x05, // syscall
            0xBF, 0x34, 0x12, 0x00, 0x00, // mov edi, 0x1234
            0xB8, 0x01, 0x00, 0x00, 0x00, // mov eax, 1
            0x0F, 0x05, // syscall
            0xF4, // hlt
        ];
        core::ptr::copy_nonoverlapping(program.as_ptr(), code_bytes, program.len());
        vmm::map_page_in(space, USER_CODE_VADDR, code_page, vmm::PAGE_USER);
        // deliberately no PAGE_NO_EXECUTE -- this page must be executable

        let stack_page = pmm::alloc_page();
        vmm::map_page_in(
            space,
            USER_STACK_VADDR,
            stack_page,
            vmm::PAGE_USER | vmm::PAGE_NO_EXECUTE | vmm::PAGE_WRITABLE,
        );

        // Uses THIS thread's own kernel stack (thread::spawn already
        // allocated it) for both TSS.RSP0 and syscall.rs's KERNEL_RSP,
        // not a separate scratch buffer — see
        // thread::current_kernel_stack_top's doc comment for the real bug
        // that reusing a shared scratch stack caused once syscalls became
        // preemptible (necessary for a blocking syscall like #2 below to
        // avoid deadlocking on interrupts-disabled).
        let kernel_stack_top = thread::current_kernel_stack_top();
        gdt::set_kernel_stack(kernel_stack_top);
        syscall::set_kernel_stack(kernel_stack_top);
        syscall::init();
        syscall::init_ring3_ipc_demo();
        thread::spawn(ring3_ipc_receiver);

        vmm::switch_address_space(space);
        thread::set_current_address_space(space);
        klog_info!(
            "RING3_ENTER entry=0x{:x} stack=0x{:x}",
            USER_CODE_VADDR,
            USER_STACK_VADDR + 4096
        );
        ring3::enter_user_mode(USER_CODE_VADDR, USER_STACK_VADDR + 4096);
    }
}

/// Kernel-side counterpart to syscall number 2 (syscall.rs): waits for the
/// message a REAL ring-3 process sends via the syscall surface, proving
/// the data genuinely crosses the ring3->syscall->IPC->kernel-thread path,
/// not just that the syscall returned success.
#[cfg(feature = "demo_ring3")]
extern "C" fn ring3_ipc_receiver() {
    let table = syscall::ring3_ipc_table();
    let cap = syscall::ring3_send_cap();
    match ipc::receive(table, cap) {
        Ok(msg) => klog_info!("RING3_IPC_RECEIVER got 0x{:x} (via syscall from ring 3)", msg.data[0]),
        Err(e) => klog_info!("RING3_IPC_RECEIVER FAILED {:?}", e),
    }
}

extern "C" fn demo_thread_b() {
    for i in 0..5u32 {
        klog_info!("THREAD_B tick={}", i);
        for _ in 0..80_000_000u64 {
            unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
        }
    }
    klog_info!("THREAD_B_DONE");
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    if let Some(location) = info.location() {
        klog_error!(
            "RUST PANIC at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        );
    } else {
        klog_error!("RUST PANIC (no location info)");
    }
    loop {
        unsafe {
            core::arch::asm!("cli; hlt", options(nomem, nostack));
        }
    }
}
