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
pub mod gdt;
pub mod heap;
pub mod idt;
pub mod klog;
pub mod pic;
pub mod pmm;
pub mod serial;
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

    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
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
