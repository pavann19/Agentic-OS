//! Deliberate fault triggers, each behind its own Cargo feature — never
//! compiled into a normal build. `docs/ROADMAP.md` Phase 0's fault
//! injection requirement: a null dereference, a write to read-only kernel
//! text/rodata, an execute attempt on NX data, and a double fault must
//! each produce a diagnosable panic, not a silent hang. This is what
//! `make test-faults` builds and boots, one variant per feature.
//!
//! Called from `main.rs` right after VMM_INIT_DONE — every one of these
//! needs the real (not bootstrap) page tables active to actually prove
//! anything; triggering a "null deref" against the bootstrap identity map
//! wouldn't fault at all, since low physical/virtual page 0 could easily
//! be identity-mapped there.

#[cfg(feature = "fault_test_null_deref")]
pub fn run() -> ! {
    crate::klog_info!("FAULT_INJECTION: null_deref — reading address 0");
    unsafe {
        let value = core::ptr::read_volatile(0 as *const u64);
        // Should never reach here — if it does, the null guard is broken.
        crate::klog_info!("FAULT_INJECTION_FAILURE: read 0x{:x} from address 0, expected a fault", value);
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

#[cfg(feature = "fault_test_rodata_write")]
pub fn run() -> ! {
    extern "C" {
        static __rodata_start: u8;
    }
    crate::klog_info!("FAULT_INJECTION: rodata_write — writing to kernel .rodata");
    unsafe {
        // addr_of! rather than `&__rodata_start as *const u8` deliberately:
        // taking a shared reference and then writing through it (even via
        // write_volatile) is UB per Rust's aliasing model and is now a
        // hard compile error on this nightly ("assigning to `&T` is
        // undefined behavior") -- addr_of! gets the address without ever
        // forming a reference at all, which is exactly what's needed here
        // since deliberately faulting on the write is the whole point.
        let addr = core::ptr::addr_of!(__rodata_start) as *mut u8;
        core::ptr::write_volatile(addr, 0xFF);
        crate::klog_info!("FAULT_INJECTION_FAILURE: wrote to .rodata without faulting");
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

#[cfg(feature = "fault_test_nx_exec")]
pub fn run() -> ! {
    crate::klog_info!("FAULT_INJECTION: nx_exec — executing a byte in .data");
    // 0xC3 = `ret`. Lives in .data (writable, mapped NX) — this is
    // deliberately the same failure mode the real bug this session found
    // and fixed produced by accident (instruction fetch on an NX page).
    static mut CODE_IN_DATA: [u8; 8] = [0xC3, 0, 0, 0, 0, 0, 0, 0];
    unsafe {
        let f: extern "C" fn() = core::mem::transmute(CODE_IN_DATA.as_ptr());
        f();
        crate::klog_info!("FAULT_INJECTION_FAILURE: executed .data without faulting");
    }
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

#[cfg(feature = "fault_test_double_fault")]
pub fn run() -> ! {
    crate::klog_info!("FAULT_INJECTION: double_fault — recursing past the mapped stack region");
    recurse(0);
    crate::klog_info!("FAULT_INJECTION_FAILURE: recursion returned without faulting");
    loop {
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

#[cfg(feature = "fault_test_double_fault")]
#[inline(never)]
fn recurse(n: u64) -> u64 {
    // A large stack frame so this blows through the mapped stack region
    // (vmm.rs maps 6MB around the captured boot-time RSP) in a bounded
    // number of calls rather than needing millions of tiny frames — the
    // page fault this hits while ALREADY handling the fault from running
    // out of stack is exactly the double-fault condition the dedicated
    // IST stack (gdt.rs) exists for.
    //
    // Found by testing, not anticipated: the first version wrote
    // `recurse(sum + 1)` as the final (tail-position) expression, and
    // release-mode LLVM turned the self-tail-call into a loop with
    // CONSTANT stack usage (confirmed via `qemu -d int`: 399 real timer
    // interrupts fired over 15s with zero page faults — the recursion was
    // never actually growing the stack at all). `black_box` plus doing
    // real work with the result AFTER the call — `1 +
    // core::hint::black_box(recurse(...))` — forces the call out of tail
    // position, so each level's stack frame must stay live until the
    // call returns.
    let padding = core::hint::black_box([n; 4096]);
    let sum: u64 = padding.iter().sum();
    1 + core::hint::black_box(recurse(sum + 1))
}
