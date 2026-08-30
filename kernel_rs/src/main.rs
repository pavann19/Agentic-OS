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
pub mod apic;
pub mod audit;
pub mod bootinfo;
pub mod capability;
pub mod device_manager;
pub mod driver;
pub mod events;
pub mod interrupt_forward;
pub mod ipc;
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
pub mod pci;
pub mod pic;
pub mod pmm;
#[cfg(feature = "demo_ring3")]
pub mod ring3;
pub mod serial;
pub mod syscall;
pub mod thread;
pub mod vmm;

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
    gdt::init();
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
    klog_info!("TIMER_INIT_START");
    apic::init();
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
    klog_info!("TIMER_INIT_DONE");

    klog_info!("Rust kernel slice: PMM+VMM+heap+timer+deferred-IRQ-queue live, higher-half.");
    klog_info!("NOTE: graphics/keyboard not yet ported (Phase 0 core items complete).");

    // Phase 3: real PCIe enumeration against whatever this machine
    // actually has, not a synthetic/mocked device list.
    klog_info!("PCI_ENUMERATE_START");
    let pci_devices = pci::enumerate();
    pci::log_all(&pci_devices);
    klog_info!("PCI_ENUMERATE_DONE count={}", pci_devices.len());

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
    // Bring the real SATA/AHCI controller (00:1f.2) up to Running, then
    // deliberately simulate a crash and recovery on it, to prove the
    // restart-on-crash state machine against a real device entry rather
    // than a synthetic one -- see device_manager.rs's honesty note: this
    // proves the state machine, not that a real driver actually crashed.
    devmgr.mark_running(0, 0x1f, 2);
    let restarted = devmgr.report_crash(0, 0x1f, 2);
    klog_info!("DEVMGR_SIMULATED_CRASH device=00:1f.2 restart_scheduled={}", restarted);
    if restarted {
        devmgr.mark_running(0, 0x1f, 2);
    }
    devmgr.log_summary();
    klog_info!("DEVMGR_INIT_DONE");

    // Phase 3: ACPI table discovery -- the real RSDP boot_rs found via the
    // UEFI configuration table, walked to find DMAR (the IOMMU's register
    // base) for the item below.
    klog_info!("ACPI_INIT_START rsdp=0x{:x}", info.payload.rsdp as u64);
    let xsdt = acpi::init(info.payload.rsdp as u64);
    match xsdt {
        Some(xsdt_phys) => {
            klog_info!("ACPI_INIT_DONE xsdt=0x{:x}", xsdt_phys);
            match acpi::find_table(xsdt_phys, b"DMAR") {
                Some(dmar_phys) => {
                    klog_info!("ACPI_DMAR_FOUND phys=0x{:x}", dmar_phys);
                    klog_info!("IOMMU_INIT_START");
                    if iommu::init(dmar_phys) {
                        klog_info!("IOMMU_INIT_DONE");
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

    // Phase 1: kernel threads + preemptive scheduling. kernel_main itself
    // becomes "thread 0" (its execution continues below exactly as
    // before — this call just makes it visible to the scheduler so
    // h_timer's schedule() has a valid thread to save/restore starting on
    // the very next tick).
    thread::init_as_current_thread();
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
