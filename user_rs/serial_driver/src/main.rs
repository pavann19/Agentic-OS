//! Real first user-space driver (Phase 3): a genuine standalone ELF64
//! binary, not a hand-built machine-code byte array like every ring-3
//! process before it (`init.rs`, the Phase 1/2 `demo_ring3` proof).
//! Loaded and mapped by `kernel_rs/src/elf.rs`'s real ELF loader.
//!
//! What this proves, concretely: the kernel granted this process a
//! `PortIoRange` capability for COM1 (0x3F8-0x3FF) BEFORE entering ring
//! 3 (`driver.rs::grant_port_access`, which opens exactly those bits in
//! the TSS IOPB — see that module's doc comment for why this is real
//! per-port mediation, not `IOPL=3`). This program does the actual
//! hardware I/O itself, completely unmediated by the kernel after that
//! one-time grant — genuine ring-3 `out`/`in` instructions against COM1,
//! exactly matching real x86 IOPB semantics (the CPU checks the bitmap
//! in hardware on every I/O instruction; there is no kernel code running
//! per-byte). The bytes it writes land in the SAME COM1 UART the
//! kernel's own `serial.rs` uses for every `[INFO]`/`[ERROR]` log line —
//! so a real string appearing in `_evidence/latest/serial.log`, written
//! by code that never went through `klog_info!`, is direct, physical
//! evidence this ran as real ring-3 code with real hardware access, not
//! a simulated log line.
//!
//! No process-exit syscall exists yet (a real, open gap — nothing in
//! this kernel has a clean process-exit path from ring 3 yet), so this
//! ends in a benign infinite spin rather than the deliberate
//! privileged-instruction #GP the OTHER ring-3 demos use to prove CPL —
//! that deliberate-crash technique halts the WHOLE kernel today (Phase
//! 1's still-open "one exception halts the machine" gap), which is fine
//! for a one-shot proof-of-mechanism demo but wrong for something meant
//! to represent a real, ongoing driver.
//!
//! Phase 13 deliverable 3: this is the first crate migrated to
//! `agentic_sdk` (new) instead of hand-rolling its own COM1/syscall
//! code — the real proof that crate is a genuine drop-in, not just an
//! untested library. Behavior is byte-identical: same COM1 output, same
//! syscall 1 with the same real full-clobber-list fix this crate's own
//! doc originally described (now `agentic_sdk::syscall::syscall3`'s
//! one canonical implementation instead of being re-derived here).

#![no_std]
#![no_main]

#[no_mangle]
pub extern "C" fn _start() -> ! {
    agentic_sdk::com1::write_str("\n[USERSPACE_SERIAL_DRIVER] real ELF64 ring-3 process, real PortIoRange capability, real COM1 write\n");
    // "D067" ~ "driver" -- distinguishes this real-ELF process's syscall
    // from the hand-built demos' markers (0xCAFE, 0x1234, 0xC0DE).
    unsafe { agentic_sdk::syscall::syscall1(1, 0xD067) };

    // The "one measured systems metric" the original CI/evidence plan
    // called for (docs/PERFORMANCE_BASELINE.md) -- real syscall
    // round-trip latency, added here because this process is the
    // earliest real ring-3 ELF spawned on every boot, feature-flag or
    // not, so the number is present in every test's serial log, not
    // gated behind a demo build.
    //
    // Benchmarks syscall 1 (SYSCALL_LOG, the same one used two lines
    // up), NOT SYS_YIELD (29) -- SYS_YIELD was tried first and produced
    // wildly inflated, wildly noisy numbers (hundreds of millions of
    // cycles), because it does exactly what its name says: it can hand
    // the rest of this thread's quantum to another ready thread and not
    // return until rescheduled. This early in boot, several other
    // drivers are actively spawning, so that handoff is real and long
    // -- a genuine measurement, just of scheduler latency, not syscall
    // dispatch cost. Syscall 1 never voluntarily gives up the CPU, so
    // its round trip isolates SYSCALL/SYSRET entry+exit plus one real
    // kernel-side COM1 PIO write -- a real, disclosed, non-zero amount
    // of kernel work, not a bare no-op, but a consistent one that
    // doesn't depend on what every other thread happens to be doing.
    //
    // Sample count is deliberately small: this process gets exactly one
    // mapped 4KB page for its ring-3 stack (`compositor.rs`'s
    // `DRIVER_STACK_VADDR` pattern, one page, no guard growth) -- a
    // first attempt at 2000 samples (16000 bytes on the stack) blew
    // straight through it and page-faulted before the first sample was
    // even taken, silently killing this process. 63 samples (504 bytes)
    // leaves comfortable headroom and is still a real, sorted, N=63
    // population to take a genuine median from.
    //
    // Built as an array of `MaybeUninit<u64>`, filled ONLY by individual
    // scalar element writes -- never through a bulk `[0u64; N]` literal
    // or a `core::array::from_fn`-style build-then-copy. Both of those
    // were tried first and both faulted: `[0u64; N]` is exactly the
    // bulk-zero-init pattern `kernel_common::mem_intrinsics`'s own
    // module doc describes LLVM lowering into a `memset` call on this
    // toolchain, and `from_fn` builds its result in a temporary before
    // copying it into `samples`, which LLVM lowered into a `memcpy`
    // call instead -- SAME underlying bug, different intrinsic. Both
    // went through the broken indirect-call-through-a-null-import-slot
    // path (cr2=0x0 page faults) even with `provide_mem_intrinsics`
    // linked in, so that existing fix does not cover every call site
    // LLVM can choose for a ~500-byte block operation. `MaybeUninit`
    // sidesteps this rather than chasing the toolchain bug further:
    // `MaybeUninit::uninit()` emits no store at all, and every element
    // below is written individually, so no memset/memcpy-sized block
    // operation is ever generated for this array, only scalar stores.
    const LATENCY_SAMPLES: usize = 63;
    let mut samples: [core::mem::MaybeUninit<u64>; LATENCY_SAMPLES] =
        unsafe { core::mem::MaybeUninit::uninit().assume_init() };
    for slot in samples.iter_mut() {
        let start = agentic_sdk::timing::read_tsc();
        unsafe { agentic_sdk::syscall::syscall1(1, 0) };
        let end = agentic_sdk::timing::read_tsc();
        slot.write(end.saturating_sub(start));
    }
    // SAFETY: every slot was written by the loop directly above, so
    // every `assume_init()` read from here on is of real initialized
    // data.
    // Insertion sort -- no allocator in this no_std ring-3 binary, and
    // 63 elements is trivial for O(n^2) at this one-shot scale.
    for i in 1..LATENCY_SAMPLES {
        let key = unsafe { samples[i].assume_init() };
        let mut j = i;
        while j > 0 && unsafe { samples[j - 1].assume_init() } > key {
            let prev = unsafe { samples[j - 1].assume_init() };
            samples[j].write(prev);
            j -= 1;
        }
        samples[j].write(key);
    }
    let min_cycles = unsafe { samples[0].assume_init() };
    let max_cycles = unsafe { samples[LATENCY_SAMPLES - 1].assume_init() };
    let median_cycles = unsafe { samples[LATENCY_SAMPLES / 2].assume_init() };
    // Nominal ~2.5GHz QEMU/TCG clock -- the same unverified constant
    // `kernel_rs::compositor_metrics::CYCLES_PER_US` already uses for
    // its own reported microsecond figures. This is a readability
    // estimate, not a calibrated wall-clock measurement; the cycle
    // counts above are the real, exact numbers this run produced.
    let median_ns_est = median_cycles * 2 / 5; // cycles / 2500 * 1000
    agentic_sdk::com1::write_str("[SYSCALL_LATENCY] samples=");
    agentic_sdk::com1::write_dec_u64(LATENCY_SAMPLES as u64);
    agentic_sdk::com1::write_str(" median_cycles=");
    agentic_sdk::com1::write_dec_u64(median_cycles);
    agentic_sdk::com1::write_str(" min_cycles=");
    agentic_sdk::com1::write_dec_u64(min_cycles);
    agentic_sdk::com1::write_str(" max_cycles=");
    agentic_sdk::com1::write_dec_u64(max_cycles);
    agentic_sdk::com1::write_str(" median_ns_est=");
    agentic_sdk::com1::write_dec_u64(median_ns_est);
    agentic_sdk::com1::write_str(" note=measured_under_qemu_tcg_emulation_not_real_hardware_ns_is_nominal_2500cyc_per_us_estimate\n");

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
