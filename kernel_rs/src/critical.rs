//! Critical-section helper for single-core interrupt-safety. Real bug
//! this exists to fix (found by bisecting a reproducible late-boot crash
//! under sustained multi-process scheduling — see PROGRESS.md's Phase 3
//! section for the full investigation): `pmm.rs`'s physical-page bitmap
//! allocator and `heap.rs`'s free-list allocator both mutate global
//! state via a check-then-mutate sequence that ran with interrupts
//! enabled the whole time. `heap.rs`'s own module doc already flagged
//! this exact gap explicitly — "Not thread/interrupt-safe by itself...
//! revisit with real locking before [Phase 1's scheduler] does [sustained
//! alloc/free churn]" — Phase 1 landed and that revisit never happened
//! until now.
//!
//! Two distinct real failure modes, both closed by the same fix:
//!
//! 1. **Torn allocation** (`pmm::alloc_page`): the bitmap scan
//!    (`bitmap_test` returns false — "page N is free") and the mark
//!    (`bitmap_set` — "page N is now used") are two separate steps. A
//!    timer interrupt landing between them lets a SECOND thread's own
//!    `alloc_page()` call see the SAME pre-mark bitmap state, conclude
//!    page N is ALSO free, and hand out the SAME physical page to two
//!    unrelated callers — silently aliasing two different page tables
//!    (or a page table and something else entirely) onto one physical
//!    frame. Whichever caller writes to it last corrupts the other's
//!    view of it — exactly consistent with a live, previously-working
//!    process's own page mapping later reporting not-present with no
//!    code ever calling `unmap_page` on it.
//! 2. **Self-deadlock via preempt-while-locked** (`heap.rs`'s
//!    `SpinMutex`): a spinlock alone is NOT sufficient mutual exclusion
//!    on a single core with PREEMPTIVE scheduling. If thread A acquires
//!    `ALLOCATOR`'s lock and gets preempted mid-mutation (lock still
//!    held), and `schedule()` itself — running inside the very timer
//!    interrupt that preempted A, with interrupts hardware-disabled by
//!    the interrupt-gate entry — ever needs to allocate (e.g. the
//!    scheduler's own `VecDeque<Box<Thread>>` growing), it would spin
//!    forever on a lock that can only ever be released by resuming
//!    thread A, which requires exactly the interrupt context currently
//!    stuck spinning to ever return. A true deadlock, not just extra
//!    spinning.
//!
//! Disabling interrupts around each critical section closes BOTH: no
//! preemption can occur while the section runs, so no other thread can
//! ever observe (or contend for) a partially-mutated structure, and
//! `schedule()` can never be invoked while any of these locks are held —
//! the standard technique real single-core kernels use instead of a bare
//! spinlock for exactly this reason (the "irqsave" half of Linux's
//! `spin_lock_irqsave`, applied here without needing SMP's actual lock
//! half since there is only one core to exclude).

/// Runs `f` with interrupts disabled, restoring the PREVIOUS
/// interrupts-enabled state on exit (not unconditionally re-enabling) —
/// composes correctly whether called from normal thread context
/// (interrupts on), from inside another `without_interrupts` (already
/// off — this becomes a no-op restore), or from early boot before the
/// first `sti` ever runs (also already off).
#[inline]
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let flags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {0}",
            "cli",
            out(reg) flags,
            options(nomem, preserves_flags)
        );
    }
    let result = f();
    if flags & 0x200 != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
    result
}
