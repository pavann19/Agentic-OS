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

pub mod apic;
pub mod bootinfo;
pub mod events;
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
pub mod klog;
pub mod pic;
pub mod pmm;
pub mod serial;
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
