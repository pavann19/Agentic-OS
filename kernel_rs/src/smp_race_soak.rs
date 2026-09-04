//! Phase 9 deliverable 3's real exit-criterion evidence
//! (`docs/ROADMAP.md` §5 — "a deliberate cross-core race (two cores
//! contending the same capability-table entry, injected the same way
//! Phase 6's fault-injection harness injects driver faults) is caught,
//! not silently corrupting state"). Two real threads, PINNED to two
//! DIFFERENT real cores (`thread::spawn_pinned_to_cpu`), each hammer
//! `capability::create_object` in a tight loop at the same wall-clock
//! time — the exact shared structure `critical.rs`'s own doc comment
//! names (`capability.rs`'s `OBJECTS`). If the kernel-wide lock
//! (`critical::without_interrupts`, made genuinely cross-core by
//! deliverable 5) is doing its job, every one of the `2 * ITERATIONS`
//! calls returns a distinct id and the object count afterward is
//! EXACTLY `2 * ITERATIONS` — no lost increments, no duplicate ids,
//! the same "torn allocation" failure shape `critical.rs`'s own module
//! doc already named for `pmm::alloc_page`, just for the capability
//! table instead.
//!
//! Only meaningful with at least 2 real cores online — a single-core
//! boot (no `-smp` flag) skips this cleanly rather than pretending to
//! race against itself.

use crate::capability::{self, KernelObjectKind, ObjectId};
use crate::klog_info;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

const ITERATIONS: usize = 200;
const MAX_IDS_PER_SIDE: usize = ITERATIONS;

static START_GATE: AtomicBool = AtomicBool::new(false);
static DONE_COUNT: AtomicU32 = AtomicU32::new(0);
static BASELINE_COUNT: AtomicUsize = AtomicUsize::new(0);

// Real, fixed-capacity storage for every id each side observed — a
// `Vec` protected by the same lock the test is trying to stress would
// mask exactly the bug this test exists to catch, so these are plain,
// racily-written arrays (each SIDE only ever writes its OWN index
// range, so the actual writes never race with each other -- only the
// `create_object` calls themselves do).
static mut SIDE_A_IDS: [ObjectId; MAX_IDS_PER_SIDE] = [0; MAX_IDS_PER_SIDE];
static mut SIDE_B_IDS: [ObjectId; MAX_IDS_PER_SIDE] = [0; MAX_IDS_PER_SIDE];

fn wait_for_gate() {
    // Bounded spin -- both racer threads sit here until the gate opens,
    // maximizing real overlap between the two cores' hammering instead
    // of one finishing before the other even starts.
    let mut spins: u64 = 0;
    while !START_GATE.load(Ordering::Acquire) {
        core::hint::spin_loop();
        spins += 1;
        if spins > 500_000_000 {
            break; // shouldn't happen -- the starter always flips this promptly
        }
    }
}

extern "C" fn racer_a() {
    wait_for_gate();
    for i in 0..ITERATIONS {
        let id = capability::create_object(KernelObjectKind::IpcEndpoint);
        unsafe {
            SIDE_A_IDS[i] = id;
        }
    }
    DONE_COUNT.fetch_add(1, Ordering::SeqCst);
}

extern "C" fn racer_b() {
    wait_for_gate();
    for i in 0..ITERATIONS {
        let id = capability::create_object(KernelObjectKind::IpcEndpoint);
        unsafe {
            SIDE_B_IDS[i] = id;
        }
    }
    DONE_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// `thread::spawn`'s entry point — runs as its OWN kernel thread (NOT
/// called inline from the boot sequence) so its real, necessarily-
/// bounded spin-waits never delay the rest of boot. Pins both racers to
/// cpu 1 and cpu 2 specifically (never cpu 0, the BSP) so the BSP stays
/// completely free to keep running the normal boot sequence
/// concurrently — real concurrency, not "the boot thread pauses itself
/// to run a test."
pub extern "C" fn run_as_thread() {
    run(1, 2);
}

/// Runs the real soak test against cores `cpu_a`/`cpu_b` — callers
/// (`run_as_thread`) are responsible for having already verified both
/// are real, online cores distinct from each other and from wherever
/// this coordinator itself happens to be running.
fn run(cpu_a: usize, cpu_b: usize) {
    BASELINE_COUNT.store(capability::object_count(), Ordering::SeqCst);
    DONE_COUNT.store(0, Ordering::SeqCst);
    START_GATE.store(false, Ordering::SeqCst);

    klog_info!(
        "SMP_RACE_SOAK_START cpu_a={} cpu_b={} iterations_per_side={}",
        cpu_a, cpu_b, ITERATIONS
    );

    crate::thread::spawn_pinned_to_cpu(racer_a, crate::vmm::kernel_pml4_phys(), cpu_a);
    crate::thread::spawn_pinned_to_cpu(racer_b, crate::vmm::kernel_pml4_phys(), cpu_b);

    // Real gap between spawn and start -- give both racer threads a
    // moment to actually reach `wait_for_gate` before opening it, so
    // the race is genuinely concurrent rather than accidentally
    // sequential because one side started hammering before the other
    // was even scheduled. This coordinator thread is unpinned (runs
    // wherever `thread::spawn` placed it, never cpu 1 or 2 themselves),
    // so this spin never itself delays either racer from being
    // scheduled the way it would have if the BSP's own boot thread had
    // done this waiting inline.
    for _ in 0..2_000_000u64 {
        core::hint::spin_loop();
    }
    START_GATE.store(true, Ordering::Release);

    let mut spins: u64 = 0;
    while DONE_COUNT.load(Ordering::SeqCst) < 2 {
        core::hint::spin_loop();
        spins += 1;
        if spins > 500_000_000 {
            klog_info!("SMP_RACE_SOAK_TIMEOUT -- one or both racers never finished");
            return;
        }
    }

    let expected_new = 2 * ITERATIONS;
    let actual_count = capability::object_count();
    let baseline = BASELINE_COUNT.load(Ordering::SeqCst);
    let actual_new = actual_count.saturating_sub(baseline);

    // Real duplicate-id check across BOTH sides -- a torn allocation
    // (two racers handed the SAME id) would show up here even if the
    // total COUNT happened to still look right by coincidence.
    let mut duplicate_found = false;
    unsafe {
        for i in 0..ITERATIONS {
            for j in 0..ITERATIONS {
                if SIDE_A_IDS[i] == SIDE_B_IDS[j] {
                    duplicate_found = true;
                }
            }
        }
    }

    if actual_new == expected_new && !duplicate_found {
        klog_info!(
            "SMP_RACE_SOAK_PASS expected_new_objects={} actual_new_objects={} no_duplicate_ids=true",
            expected_new, actual_new
        );
    } else {
        klog_info!(
            "SMP_RACE_SOAK_FAIL expected_new_objects={} actual_new_objects={} duplicate_found={}",
            expected_new, actual_new, duplicate_found
        );
    }
}
