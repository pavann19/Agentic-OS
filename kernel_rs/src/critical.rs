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
//! Disabling interrupts around each critical section closed BOTH failure
//! modes for as long as there was exactly one core — no preemption could
//! occur while the section ran, so no other execution context could ever
//! observe (or contend for) a partially-mutated structure. That was
//! ALWAYS only half of Linux's `spin_lock_irqsave` (the "irqsave" half);
//! the other half (an actual lock, excluding OTHER CORES) was
//! deliberately not needed while this kernel was single-core.
//!
//! Phase 9 deliverable 5 (`docs/ROADMAP.md` §5 — "every existing shared
//! kernel structure from Phases 0-8 audited and made SMP-safe: the
//! capability table, the audit log, IOMMU domain assignment, the
//! PMM/heap"): once a second real core can execute concurrently
//! (deliverable 1), interrupts disabled on core 0 do nothing to stop
//! core 1 from touching the same static at the same wall-clock instant.
//! Every one of the structures deliverable 5 names — `pmm.rs`'s bitmap,
//! `capability.rs`'s `OBJECTS`, `audit.rs`'s `LOG`, `iommu.rs`'s domain/
//! context-table state, plus `thread.rs`'s own scheduler state and
//! several more not explicitly named (`object_store.rs`, `policy.rs`,
//! `device_manager.rs`, `service_manager.rs`, `ipc.rs`,
//! `interrupt_forward.rs`, `introspect.rs`) — already routes every one
//! of its mutations through THIS function, uniformly, because that was
//! already the established pattern for "protect shared kernel state"
//! across this whole codebase. That uniformity is what makes fixing it
//! HERE, once, a genuine fix for all of them at once, rather than a
//! per-file sweep this session could plausibly miss a call site in.
//!
//! **Deliberately a single, coarse-grained kernel lock (a real,
//! historically-precedented design, not a shortcut):** every critical
//! section in the kernel now excludes every OTHER core too, not just
//! this one's own interrupts — genuinely correct, not just
//! single-core-correct. It is coarser than per-structure locks would be
//! (a capability-table mutation on core 0 now also blocks an unrelated
//! audit-log write on core 1) — a real, stated scalability cost, exactly
//! the trade-off early SMP Linux (2.0/2.2's "Big Kernel Lock") made
//! deliberately for the same reason: correctness first, contention
//! later, once real measurement shows it matters. Splitting this into
//! genuinely independent per-structure locks is real, tracked future
//! work — not a correctness gap today.
//!
//! **Recursive by construction — the one real hazard a naive spinlock
//! swap-in would have hit:** several existing call sites nest (e.g.
//! `audit.rs::record`'s own doc comment: "nesting is safe... `actor_tid`
//! reads `thread::current_id()`, which takes its own lock internally").
//! That was true for a bare `cli`/`sti` pair (idempotent) but would
//! DEADLOCK a naive non-reentrant spinlock the instant the same core
//! tried to re-acquire a lock it already holds. Tracking the owning
//! core's index and a reentry depth (both below) makes re-entry from the
//! SAME core a cheap no-op, while a DIFFERENT core still genuinely spins
//! until the owner's outermost call releases it.

use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};

/// -1 = unlocked; otherwise the `smp::current_cpu_index()` of whichever
/// core currently holds the lock (real hardware-APIC-ID-resolved
/// identity, not a guess).
static KERNEL_LOCK_OWNER: AtomicI64 = AtomicI64::new(-1);
/// Per-CPU reentry depth: each core tracks its own nesting depth independently.
/// This prevents cross-core races where depth drops to 0 before owner is cleared,
/// and eliminates multi-core de-synchronization during reentrant lock acquisition.
static PER_CPU_DEPTH: [AtomicU32; crate::smp::MAX_CPUS] = [const { AtomicU32::new(0) }; crate::smp::MAX_CPUS];

// Imbalance diagnostics, added while chasing a real, open, CI-only
// non-deterministic page fault (see docs/VERIFICATION.md's Known
// Issues section): the live hypothesis is a fault landing between
// acquire() succeeding and release() running, leaving the lock held
// forever. TOTAL_ACQUIRES/TOTAL_RELEASES only increment on the
// outermost (depth 0->1 / 1->0) transition, so on a healthy system
// they track each other 1:1 -- a growing gap is direct evidence of a
// leaked lock. release() at depth==0 used to be a silent no-op; that
// silently hides exactly the bug being hunted, so it's now counted
// and logged instead.
static TOTAL_ACQUIRES: AtomicU64 = AtomicU64::new(0);
static TOTAL_RELEASES: AtomicU64 = AtomicU64::new(0);
static UNBALANCED_RELEASES: AtomicU64 = AtomicU64::new(0);

/// Real diagnostic snapshot for `idt::recover_or_halt` to log at the
/// exact moment of a ring-0 fault -- if `owner != -1` or
/// `depth_here > 0` right then, the lock was genuinely held (by
/// someone) when the fault hit, which is what a real acquire/release
/// imbalance around the faulting code would look like.
pub fn diag_snapshot() -> (i64, u32, u64, u64, u64) {
    let me = crate::smp::current_cpu_index().min(crate::smp::MAX_CPUS - 1);
    (
        KERNEL_LOCK_OWNER.load(Ordering::Relaxed),
        PER_CPU_DEPTH[me].load(Ordering::Relaxed),
        TOTAL_ACQUIRES.load(Ordering::Relaxed),
        TOTAL_RELEASES.load(Ordering::Relaxed),
        UNBALANCED_RELEASES.load(Ordering::Relaxed),
    )
}

/// Runs `f` with interrupts disabled AND this core holding the one
/// real, kernel-wide lock (see this module's own doc comment for the
/// full design and why it's deliberately coarse-grained and
/// deliberately recursive). Restores the PREVIOUS interrupts-enabled
/// state on exit (not unconditionally re-enabling) — composes correctly
/// whether called from normal thread context (interrupts on), from
/// inside another `without_interrupts` on the SAME core (already held —
/// becomes a cheap depth-counted no-op on the lock itself, still
/// correctly cli/sti-nested), from a DIFFERENT core genuinely contending
/// the same section (spins, bounded, until the owner's outermost call
/// releases it), or from early boot before the first `sti` ever runs
/// (interrupts already off, `smp::current_cpu_index()` resolves to 0 —
/// correct, since only the BSP is ever running that early).
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

    acquire();
    let result = f();
    release();

    if flags & 0x200 != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
    result
}

/// The acquire half of `without_interrupts`, split out so
/// `thread.rs::schedule_locked` can manage the lock EXPLICITLY around
/// its own real-mode-switch boundary (`switch_to`) — see this module's
/// own doc comment addendum below for exactly why the normal
/// closure-scoped `without_interrupts` cannot be used there. Callers
/// MUST have already disabled interrupts (`cli`) themselves — this
/// function only ever touches the lock's owner/depth bookkeeping, never
/// RFLAGS.
#[inline]
pub fn acquire() {
    let me = crate::smp::current_cpu_index().min(crate::smp::MAX_CPUS - 1);
    let me_i64 = me as i64;
    let depth = PER_CPU_DEPTH[me].load(Ordering::Relaxed);

    if depth > 0 {
        // Already held by this core — simply increment our own nesting depth
        PER_CPU_DEPTH[me].store(depth + 1, Ordering::Relaxed);
        return;
    }

    // Lock not held by this core: spin bounded until we acquire KERNEL_LOCK_OWNER
    let mut spins: u64 = 0;
    while KERNEL_LOCK_OWNER
        .compare_exchange_weak(-1, me_i64, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
        spins += 1;
        if spins == 500_000_000 {
            crate::klog_info!("KERNEL_LOCK_STUCK cpu_index={} -- contended far longer than normal, likely a real bug", me);
        }
    }

    PER_CPU_DEPTH[me].store(1, Ordering::Relaxed);
    TOTAL_ACQUIRES.fetch_add(1, Ordering::Relaxed);
}

/// The release half — see `acquire`'s own doc comment for why
/// `thread.rs::schedule_locked` calls this directly rather than relying
/// on `without_interrupts`'s automatic closure-return cleanup.
#[inline]
pub fn release() {
    let me = crate::smp::current_cpu_index().min(crate::smp::MAX_CPUS - 1);
    let depth = PER_CPU_DEPTH[me].load(Ordering::Relaxed);
    if depth == 0 {
        // Previously a silent no-op -- masked exactly the imbalance
        // being hunted. Now counted and logged instead.
        let n = UNBALANCED_RELEASES.fetch_add(1, Ordering::Relaxed) + 1;
        crate::klog_info!("CRITICAL_UNBALANCED_RELEASE cpu_index={} count={}", me, n);
        return;
    }
    if depth == 1 {
        PER_CPU_DEPTH[me].store(0, Ordering::Relaxed);
        KERNEL_LOCK_OWNER.store(-1, Ordering::Release);
        TOTAL_RELEASES.fetch_add(1, Ordering::Relaxed);
    } else {
        PER_CPU_DEPTH[me].store(depth - 1, Ordering::Relaxed);
    }
}
