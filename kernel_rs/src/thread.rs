//! Kernel threads. Phase 1 item — nothing like this existed before (Phase 0
//! is single-threaded, one execution stream from `kernel_main` to its
//! final `hlt` loop). This is the foundation the rest of Phase 1 (per-
//! process address spaces, ring 3, syscalls, process lifecycle) builds on.
//!
//! Context-switch design: the classic "swap callee-saved registers and
//! stack pointer, then `ret`" technique (xv6/Redox/blog_os all use a
//! variant of this), not a full interrupt-frame save/restore. A thread
//! that isn't running has its entire resumption state captured as: the
//! callee-saved registers (rbx, rbp, r12-r15) plus a return address, all
//! sitting on its own kernel stack. Switching to it is just: save the
//! current callee-saved regs + RSP, load the target's RSP, restore ITS
//! callee-saved regs, `ret` — which jumps to whatever return address is on
//! top of that stack, i.e. exactly where that thread last called
//! `switch_to` from (or a trampoline, for a thread that's never run yet).
//!
//! This works correctly even when `switch_to` is called from inside the
//! timer interrupt handler (`idt.rs::h_timer`): the x86-interrupt ABI
//! already pushed the interrupted thread's SS/RSP/RFLAGS/CS/RIP onto ITS
//! OWN stack before the handler body runs. A mid-handler switch_to() swaps
//! stacks away and (later) back; when this thread is scheduled again, it
//! resumes right after switch_to(), the handler function returns
//! normally, and the compiler-generated `iretq` fires using the frame that
//! was sitting on this thread's own stack the whole time.

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use core::arch::asm;
use crate::klog_info;

pub const KERNEL_STACK_SIZE: usize = 64 * 1024;

#[repr(C)]
#[derive(Default)]
struct CalleeSaved {
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbx: u64,
    rbp: u64,
}

pub type ThreadId = u64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThreadState {
    Ready,
    Running,
    Exited,
}

pub struct Thread {
    pub id: ThreadId,
    pub state: ThreadState,
    /// Saved stack pointer — valid only while this thread is NOT running
    /// (points at the CalleeSaved frame `switch_to` pushed). While running,
    /// this is stale; the real RSP is wherever the CPU currently has it.
    saved_rsp: u64,
    _stack: Box<[u8]>, // keeps the stack allocation alive for the thread's lifetime
    /// Physical address of this thread's PML4. Kernel-only threads (the
    /// common case so far) all share `vmm::kernel_pml4_phys()` — no
    /// isolation needed between them, they're all equally trusted. A
    /// thread bound to a process (`vmm::new_address_space()`) gets its own,
    /// isolated from every other process's.
    pub address_space: u64,
    /// Phase 5: this thread's OWN capability set — "an ordinary user-space
    /// process holding a restricted capability set" (`docs/ROADMAP.md`
    /// Phase 5, deliverable 1) has to live somewhere per-process, and this
    /// is the per-process object this kernel already has. Every thread
    /// gets one (kernel threads' stays empty and unused — no real cost,
    /// an empty `Vec`). See `spawn_with_capability` for how a capability
    /// gets into a specific thread's table before it's ever reachable by
    /// the scheduler, and `resolve_current_capability` for how a syscall
    /// checks it.
    pub cap_table: crate::capability::CapabilityTable,
    /// Phase 7's real fix (found via the shell's own `rawin` demo — see
    /// `gdt.rs`'s doc comment on `set_iopb` for the full story): this
    /// thread's OWN I/O permission bitmap, not a shared global one.
    /// Starts fully denied (`[0xFF; ...]`); `driver::grant_port_access`
    /// clears bits in exactly the thread it's called from.
    pub iopb: [u8; crate::gdt::IOPB_BYTES],
}

static mut NEXT_TID: ThreadId = 1;

// Phase 9 deliverable 3 (`docs/ROADMAP.md` §5 — "SMP-safe scheduler:
// per-core run queues... IPI-based reschedule"): what used to be ONE
// global ready queue and ONE global "currently running" slot are now
// real, PER-CORE arrays, indexed by `smp::current_cpu_index()` — each
// core schedules among its OWN threads, not a single queue every core
// would otherwise contend for on every tick. Every accessor below still
// runs under `critical::without_interrupts`, which Phase 9 deliverable
// 5 upgraded into a genuine cross-core lock (see critical.rs's own doc
// comment) — the array indexing itself needs no separate lock, the
// existing discipline already covers it.
//
// Box<Thread>, not Thread, in both places: `schedule()` takes a raw
// pointer into the current thread's `saved_rsp` field BEFORE requeuing it,
// and a `VecDeque<Thread>` can move existing elements on reallocation,
// which would silently invalidate that pointer. A `VecDeque<Box<Thread>>`
// only ever moves the (small, stable-pointee) Box handle on reallocation —
// the Thread itself, and any raw pointer into its fields, stays put on the
// heap regardless. Found by review, not by a crash — worth fixing before
// it became one.
const RUN_QUEUE_INIT: Option<VecDeque<Box<Thread>>> = None;
const CURRENT_INIT: Option<Box<Thread>> = None;
static mut RUN_QUEUES: [Option<VecDeque<Box<Thread>>>; crate::smp::MAX_CPUS] =
    [RUN_QUEUE_INIT; crate::smp::MAX_CPUS];
static mut CURRENTS: [Option<Box<Thread>>; crate::smp::MAX_CPUS] = [CURRENT_INIT; crate::smp::MAX_CPUS];

// `&raw mut` + deref, not `&mut RUN_QUEUES`/`&mut CURRENTS` directly —
// the compiler's own suggested fix for the static_mut_refs lint
// everywhere else in this codebase (gdt.rs, idt.rs, pmm.rs).
// Centralized here since thread.rs's scheduler touches both statics
// from several functions. Each returns THIS core's own slot —
// `smp::current_cpu_index()` resolves real hardware APIC-ID identity,
// so a caller on core 2 can never reach through these into core 0's
// state by accident.
#[allow(static_mut_refs)]
unsafe fn threads_mut() -> &'static mut Option<VecDeque<Box<Thread>>> {
    &mut (&mut *&raw mut RUN_QUEUES)[crate::smp::current_cpu_index()]
}
#[allow(static_mut_refs)]
unsafe fn current_mut() -> &'static mut Option<Box<Thread>> {
    &mut (&mut *&raw mut CURRENTS)[crate::smp::current_cpu_index()]
}
/// Same as `current_mut`/`threads_mut`, but for an EXPLICITLY named
/// core rather than "whichever core is calling" — needed by
/// cross-core placement (`spawn_pinned_to_cpu`) and introspection
/// (`snapshot`, which must report every core's threads, not just the
/// calling core's own). Callers MUST already hold the kernel lock
/// (`critical::without_interrupts`) — these do no locking of their own,
/// same convention as `threads_mut`/`current_mut`.
#[allow(static_mut_refs)]
unsafe fn threads_mut_for(cpu: usize) -> &'static mut Option<VecDeque<Box<Thread>>> {
    &mut (&mut *&raw mut RUN_QUEUES)[cpu]
}
#[allow(static_mut_refs)]
unsafe fn current_ref_for(cpu: usize) -> &'static Option<Box<Thread>> {
    &(&*&raw const CURRENTS)[cpu]
}

/// Entry trampoline every new thread's stack is rigged to "return" into.
/// MUST be `#[unsafe(naked)]`, not a normal Rust function — found by
/// testing: a normal function gets a compiler-generated prologue (e.g.
/// pushing rbp, adjusting rsp for locals) that runs BEFORE any inline asm
/// in its body, so an inline `pop` inside a non-naked function reads
/// whatever the prologue put on the stack, not the value `spawn()`
/// actually placed there. Manifested as an instruction-fetch page fault at
/// a heap address (a stack address, misread as a return address) —
/// root-caused by reasoning through the exact byte layout `spawn()` builds
/// against what switch_to's `ret` and this function's own prologue would
/// each consume, not by guessing.
#[unsafe(naked)]
extern "C" fn thread_trampoline() -> ! {
    core::arch::naked_asm!(
        "pop rax",      // the entry fn pointer spawn() placed here
        "call rax",     // run the thread's real entry function
        "call {exit}",  // entry() returned instead of calling exit itself
        exit = sym exit_current,
    )
}

/// Builds a new thread with its own kernel stack, rigged so the first
/// `switch_to` into it starts executing `entry`. Does NOT schedule it —
/// call `enqueue` (or rely on `spawn`, which does both) separately if
/// that's not what's wanted.
pub fn spawn(entry: extern "C" fn()) -> ThreadId {
    spawn_in(entry, crate::vmm::kernel_pml4_phys())
}

/// Same as `spawn`, but binds the new thread to `address_space` (from
/// `vmm::new_address_space()`) instead of the shared kernel one —
/// `schedule()` switches CR3 automatically when consecutive threads don't
/// share an address space.
///
/// Real bug found and fixed (see `critical.rs`'s doc comment for the
/// full investigation): this function runs from ORDINARY preemptible
/// thread context (every caller in `main.rs`/`init.rs`/etc. calls it
/// after interrupts are enabled), and used to mutate `NEXT_TID` and
/// `THREADS` with no protection at all — the EXACT SAME `THREADS`
/// `VecDeque` that `schedule()` mutates from inside the timer interrupt.
/// A preemption landing mid-`push_back` here (e.g. while the VecDeque's
/// internal ring buffer is being grown/shifted) let `schedule()`, running
/// in the interrupt that preempted this call, operate on that SAME
/// half-mutated structure — corrupting it in a way that could hand a
/// LATER `pop_front()` a garbage `Box<Thread>` (a corrupted `saved_rsp`,
/// among other fields), which `switch_to`'s `ret` would then jump to.
/// `schedule()` itself doesn't need this same wrapping — it only ever
/// runs already-inside an interrupt-gate entry (hardware IF=0) until its
/// own deliberate `sti` right before `switch_to`'s `ret` — but every
/// other public function here that touches this state from normal
/// context does.
pub fn spawn_in(entry: extern "C" fn(), address_space: u64) -> ThreadId {
    crate::critical::without_interrupts(|| unsafe { spawn_in_locked(entry, address_space) })
}

unsafe fn spawn_in_locked(entry: extern "C" fn(), address_space: u64) -> ThreadId {
    unsafe {
        let tid = NEXT_TID;
        NEXT_TID += 1;

        let mut stack = alloc::vec![0u8; KERNEL_STACK_SIZE].into_boxed_slice();
        let stack_top = stack.as_mut_ptr() as u64 + KERNEL_STACK_SIZE as u64;

        // Build the initial stack frame, top-down: entry fn pointer (what
        // thread_trampoline's `pop` retrieves), then the return address
        // switch_to's `ret` will jump to (thread_trampoline itself), then
        // a zeroed CalleeSaved frame (switch_to's `pop`s just need
        // something there — all-zero callee-saved regs for a thread
        // that's never run is correct, there's no prior state to resume).
        let mut sp = stack_top;

        sp -= 8;
        *(sp as *mut u64) = entry as u64;

        sp -= 8;
        *(sp as *mut u64) = thread_trampoline as *const () as u64;

        sp -= core::mem::size_of::<CalleeSaved>() as u64;
        *(sp as *mut CalleeSaved) = CalleeSaved::default();

        let thread = Box::new(Thread {
            id: tid,
            state: ThreadState::Ready,
            saved_rsp: sp,
            _stack: stack,
            address_space,
            cap_table: crate::capability::CapabilityTable::new(),
            iopb: [0xFFu8; crate::gdt::IOPB_BYTES],
        });

        if threads_mut().is_none() {
            *threads_mut() = Some(VecDeque::new());
        }
        threads_mut().as_mut().unwrap().push_back(thread);
        tid
    }
}

/// Phase 9 deliverable 3's real, demonstrable IPI-based reschedule:
/// builds a new thread exactly like `spawn_in`, but pushes it onto
/// `target_cpu`'s OWN run queue (not the calling core's) and sends
/// that core a real reschedule IPI (`smp::send_reschedule_ipi`)
/// immediately, rather than leaving it to be picked up by that core's
/// own next periodic timer tick (bounded, but up to one full tick
/// period later). `target_cpu` is a software cpu_index
/// (`smp::current_cpu_index()`'s own numbering, 0 = BSP), not a raw
/// APIC ID — `smp::send_reschedule_ipi` does that translation.
///
/// Real, not simulated: the target core's own IDT has a handler
/// installed at `smp::RESCHEDULE_VECTOR` (`idt.rs::h_reschedule`) that
/// calls `schedule()` directly from interrupt context, the same way
/// `h_timer` already does — the only difference is WHAT triggered the
/// interrupt (another core's `send_ipi_vector`, not the local LAPIC
/// timer).
pub fn spawn_pinned_to_cpu(entry: extern "C" fn(), address_space: u64, target_cpu: usize) -> ThreadId {
    crate::critical::without_interrupts(|| unsafe {
        let tid = NEXT_TID;
        NEXT_TID += 1;

        let mut stack = alloc::vec![0u8; KERNEL_STACK_SIZE].into_boxed_slice();
        let stack_top = stack.as_mut_ptr() as u64 + KERNEL_STACK_SIZE as u64;
        let mut sp = stack_top;
        sp -= 8;
        *(sp as *mut u64) = entry as u64;
        sp -= 8;
        *(sp as *mut u64) = thread_trampoline as *const () as u64;
        sp -= core::mem::size_of::<CalleeSaved>() as u64;
        *(sp as *mut CalleeSaved) = CalleeSaved::default();

        let thread = Box::new(Thread {
            id: tid,
            state: ThreadState::Ready,
            saved_rsp: sp,
            _stack: stack,
            address_space,
            cap_table: crate::capability::CapabilityTable::new(),
            iopb: [0xFFu8; crate::gdt::IOPB_BYTES],
        });

        let q = threads_mut_for(target_cpu);
        if q.is_none() {
            *q = Some(VecDeque::new());
        }
        q.as_mut().unwrap().push_back(thread);

        crate::smp::send_reschedule_ipi(target_cpu);
        tid
    })
}

/// Same as `spawn`, but grants a SET of capabilities (`grants`, each an
/// `(object_id, rights)` pair) into the new thread's OWN `cap_table`
/// before it is ever pushed onto `THREADS` — i.e. before the scheduler
/// can possibly run it. This is what makes Phase 5's capability grant
/// race-free: `spawn_in_locked` above builds the thread and calls
/// `threads_mut().push_back` as its LAST step, all inside one
/// `critical::without_interrupts` section: a thread that isn't in that
/// queue yet cannot be picked by `schedule()` no matter when a timer
/// tick lands, so there is no window where the new thread could run
/// before holding the capabilities it's meant to start with. (Contrast:
/// granting AFTER `spawn`/`spawn_in` returns would reopen exactly that
/// race — this exists so callers never have to.)
///
/// Each grant is checked against `policy::allows()` — Phase 5's
/// grant-time policy engine — BEFORE it happens: a request for rights
/// the policy doesn't allow is refused for THAT grant specifically (an
/// `AuditEvent::PolicyDenied` record, real evidence a real grant-time
/// check ran, not silently skipped) while the thread still spawns and
/// any OTHER, policy-allowed grants in the same call still go through.
/// Grants are applied in `grants` order, so callers relying on a
/// specific `CapId` (syscalls conventionally assume slot 0, slot 1, ...
/// in grant order — see `syscall.rs`'s `AGENT_INTROSPECT_CAP`/
/// `AGENT_AUDIT_QUERY_CAP`) get a stable, predictable table layout.
pub fn spawn_with_capabilities(
    entry: extern "C" fn(),
    address_space: u64,
    grants: &[(crate::capability::ObjectId, crate::capability::Rights)],
) -> ThreadId {
    crate::critical::without_interrupts(|| unsafe {
        let tid = spawn_in_locked(entry, address_space);
        // Find the thread we just built (it's always the most recently
        // pushed one, but look it up by id rather than assume queue
        // position — cheap, and future-proof against this function ever
        // being called concurrently with itself).
        if let Some(threads) = threads_mut().as_mut() {
            if let Some(t) = threads.iter_mut().find(|t| t.id == tid) {
                for &(object_id, rights) in grants {
                    if crate::policy::allows(rights) {
                        t.cap_table.grant(object_id, rights);
                    } else {
                        crate::klog_info!("POLICY_GRANT_DENIED rights=0x{:x}", rights.0);
                        crate::audit::record(crate::audit::AuditEvent::PolicyDenied { rights: rights.0 });
                    }
                }
            }
        }
        tid
    })
}

/// Real, typed, capability-gated introspection primitive: resolves
/// `cap_id` against the CURRENTLY RUNNING thread's OWN `cap_table` — never
/// a shared global table, so two agent processes calling the same
/// syscall genuinely get judged on what THEY, individually, were
/// granted, exactly the "an agent's policy is a restriction on its own
/// capability set" model Phase 5 exists to demonstrate. Runs the whole
/// resolve under `critical::without_interrupts`, matching every other
/// CURRENT/THREADS accessor in this file — a preemption mid-resolve here
/// would otherwise risk observing a different thread's table entirely if
/// a reference leaked across a reschedule; keeping it inside one locked
/// call makes that impossible by construction.
/// Real fix for the IOPB-leak bug (see `gdt.rs`'s `set_iopb` doc):
/// clears `port`'s bit in the CURRENTLY RUNNING thread's OWN `iopb`
/// copy, then immediately pushes it into the one live TSS. The
/// immediate push matters specifically for this call site — the
/// granting thread is about to enter ring 3 for the FIRST time right
/// after this, via `ring3::enter_user_mode`, not through a
/// `schedule()` switch (which would reload it anyway) — without it,
/// that first entry would run under whatever bitmap the PREVIOUSLY
/// scheduled thread happened to leave loaded.
pub fn allow_port_for_current(port: u16) {
    crate::critical::without_interrupts(|| unsafe {
        if let Some(t) = current_mut().as_mut() {
            crate::gdt::allow_port_bits(&mut t.iopb, port);
            crate::gdt::set_iopb(&t.iopb);
        }
    });
}

/// Same as `allow_port_for_current`, for revocation — real, though not
/// yet wired to any actual revocation call site (port-capability
/// revocation was already an open, disclosed gap in `driver.rs` before
/// this fix; this makes the mechanism itself correct and ready for
/// that call site once it exists, not a promise of a feature this
/// function alone doesn't provide).
pub fn deny_port_for_current(port: u16) {
    crate::critical::without_interrupts(|| unsafe {
        if let Some(t) = current_mut().as_mut() {
            crate::gdt::deny_port_bits(&mut t.iopb, port);
            crate::gdt::set_iopb(&t.iopb);
        }
    });
}

pub fn resolve_current_capability(
    cap_id: crate::capability::CapId,
    required: crate::capability::Rights,
) -> Result<crate::capability::Capability, crate::capability::CapError> {
    crate::critical::without_interrupts(|| unsafe {
        match current_mut().as_ref() {
            Some(t) => t.cap_table.resolve(cap_id, required),
            None => Err(crate::capability::CapError::NoSuchCapability),
        }
    })
}

/// Raw asm: save callee-saved regs + RSP into `*old_rsp_slot`, load
/// `new_rsp`, switch CR3 to `new_cr3`, restore ITS callee-saved regs,
/// `ret`. Never returns to its direct caller in the normal sense —
/// control resumes wherever the target thread last left off (or
/// thread_trampoline, first time).
///
/// Real root-cause bug found and fixed (the actual explanation behind
/// the multi-process scheduling investigation in critical.rs/
/// PROGRESS.md — this is what remained after the pmm/heap/THREADS races
/// AND the TSS.RSP0 issue were all fixed and a DIFFERENT, deterministic
/// double-fault still reproduced): the CALLER used to switch CR3 itself,
/// BEFORE calling this function, while still executing ON THE OUTGOING
/// thread's OWN stack. That's harmless for every SPAWNED thread (their
/// stacks are always heap-allocated, in the shared upper half every
/// address space's PML4 copies at creation) — but thread 0 (kernel_main
/// itself, `init_as_current_thread`) runs on the ORIGINAL boot-time
/// stack, a LOW-half address that is NEVER copied into any process's own
/// PML4 (`vmm::new_address_space` only copies indices 256-511; 0-255
/// starts and stays empty, by design, for user-space isolation). The
/// instant the caller wrote a PROCESS's own CR3 while still running on
/// thread 0's low-half stack (or vice versa, switching FROM a process
/// TO thread 0), that stack became unmapped under the just-loaded page
/// tables — and the very next instruction needing to touch it (this
/// function's own prologue pushes, or the caller's own `call` pushing a
/// return address) faulted immediately: a page fault that itself
/// couldn't be delivered (the stack needed to push ITS OWN exception
/// frame was the very thing just unmapped), escalating straight to a
/// double fault. Reproduced deterministically at the exact same
/// `rip`/`rsp` every run — this is why: it's not a race, it's a genuine
/// ordering bug that fires the first time thread 0 and a process ever
/// swap directly across that CR3 boundary.
///
/// Fixed by moving the CR3 write to HERE, immediately after the RSP swap
/// (`mov rsp, rsi`) and before anything touches memory through the new
/// RSP — by the time any push/pop happens, translation already matches
/// the stack being used, regardless of which direction (thread-0-to-
/// process or process-to-thread-0) the switch goes.
#[unsafe(naked)]
unsafe extern "C" fn switch_to(old_rsp_slot: *mut u64, new_rsp: u64, new_cr3: u64) {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",   // *old_rsp_slot = current RSP (after pushes)
        "mov rsp, rsi",     // RSP = new_rsp -- now on the INCOMING thread's own stack
        "mov rax, cr3",
        "cmp rax, rdx",
        "je 2f",            // skip the TLB-flushing write if already loaded
        "mov cr3, rdx",     // translation now matches the stack just switched to
        "2:",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbx",
        "pop rbp",
        // Real bug this session found: `schedule()` always runs from
        // inside an interrupt-GATE handler (h_timer), which the CPU
        // enters with IF=0. A plain `ret` never touches RFLAGS, so
        // whatever thread we switch INTO would silently inherit
        // interrupts-disabled and could never be preempted again — only
        // escaping via something that explicitly re-enables them (which is
        // why the first version ran thread A to completion with zero
        // interleaving: exactly one `sti`, in exit_current, was the only
        // thing that ever turned interrupts back on). `sti` here,
        // immediately before `ret`, is safe due to x86's one-instruction
        // STI-shadow guarantee — the enable doesn't take effect until
        // after the NEXT instruction (this `ret`) has already jumped away,
        // so nothing can be preempted between the two.
        "sti",
        "ret",
    );
}

// Process lifecycle: reap. Holds an exited thread's Box<Thread> between
// the tick that switched it out and the NEXT tick that gets a chance to
// actually drop it. Real bug this session found by tracing through what
// switch_to() actually does: dropping an exited thread's Box INLINE,
// on the same stack frame that's about to call switch_to() and jump
// away from PERMANENTLY (nothing will ever resume that specific call),
// means the destructor never runs at all — a genuine leak of the
// thread's Box<Thread> AND its 64KB stack allocation, every single
// time any thread exits. The standard fix (matching Linux's
// finish_task_switch, conceptually): defer the drop to the NEXT
// scheduling point that runs on a DIFFERENT, still-valid stack, which
// `schedule()` reaches on every tick regardless of which thread ends
// up resumed there.
// Phase 9 deliverable 3: per-core, same reasoning as RUN_QUEUES/CURRENTS
// above. A single shared ZOMBIE slot would reopen exactly the race this
// mechanism exists to prevent, one level up: two DIFFERENT cores each
// exiting a thread in quick succession (serialized by the kernel lock,
// but not otherwise related) could overwrite each other's stashed zombie
// before either got reaped, silently leaking one Box<Thread> and its
// 64KB stack — the same "drop inline, destructor never runs" bug this
// file's own history already found and fixed once, reintroduced via a
// cross-core race instead of a same-core one.
const ZOMBIE_INIT: Option<Box<Thread>> = None;
static mut ZOMBIES: [Option<Box<Thread>>; crate::smp::MAX_CPUS] = [ZOMBIE_INIT; crate::smp::MAX_CPUS];
#[allow(static_mut_refs)]
unsafe fn zombie_mut() -> &'static mut Option<Box<Thread>> {
    &mut (&mut *&raw mut ZOMBIES)[crate::smp::current_cpu_index()]
}

/// Called from `idt.rs::h_timer` on every tick. Picks the next Ready
/// thread round-robin and switches to it; a no-op if there's nothing else
/// runnable yet (Phase 1's early state, before more than one thread
/// exists) or scheduling hasn't been initialized.
///
/// Phase 9 deliverable 5's real fix (see `critical.rs::acquire`'s own
/// doc comment for the full bug this replaced): does NOT use
/// `critical::without_interrupts`'s normal closure-scoped
/// acquire/release — `schedule_locked` below manages the kernel lock
/// EXPLICITLY, releasing it manually right before the low-level
/// `switch_to` context switch, because that call does not "return" to
/// this function in the normal sense for the outgoing thread (see
/// `critical.rs`'s doc comment for exactly why relying on
/// `without_interrupts`'s automatic cleanup there deadlocked every
/// other core the first time it happened). `cli` here is still real and
/// necessary — `schedule()` is called both from interrupt-gate context
/// (already IF=0) and from ordinary thread context
/// (`kill_current_and_reschedule`, IF=1) — this makes both cases
/// correct uniformly.
pub fn schedule() {
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
    crate::critical::acquire();
    unsafe { schedule_locked(flags) };
    // Reached ONLY on a bail-out path that released the lock and
    // restored flags itself WITHOUT switching (see the early `return`s
    // inside `schedule_locked`) -- the switching path's own `switch_to`
    // already handles both (release before switching, `sti` on
    // whichever thread resumes), so there is deliberately nothing left
    // to do here in that case.
}

/// `caller_flags` — the RFLAGS captured by `schedule()` before it
/// called `cli` — is threaded through explicitly so every EARLY-RETURN
/// path here (nothing to schedule yet, lost a work-steal race, etc.)
/// can restore it correctly before bailing, exactly mirroring what
/// `without_interrupts`'s automatic cleanup used to do. The one path
/// that reaches `switch_to` does NOT restore `caller_flags` — it
/// doesn't need to: `switch_to`'s own unconditional `sti` (its own doc
/// comment explains why) already guarantees interrupts are enabled for
/// whichever thread ends up resumed, the correct behavior regardless of
/// what `caller_flags` said (a thread should never resume with
/// interrupts disabled just because whoever LAST scheduled happened to
/// call `schedule()` from a `cli`'d context).
unsafe fn schedule_locked(caller_flags: u64) {
    // Restores `caller_flags` and releases the kernel lock -- the exact
    // pairing every early-return path below needs, extracted once
    // rather than repeated at each `return`.
    macro_rules! bail {
        () => {{
            crate::critical::release();
            if caller_flags & 0x200 != 0 {
                core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
            }
            return;
        }};
    }

    unsafe {
        // Reap whatever the PREVIOUS tick's exiting thread left behind —
        // safe here specifically because this code is running on
        // whichever thread got resumed this tick, never on the exited
        // thread's own (permanently abandoned) stack.
        if let Some(zombie) = zombie_mut().take() {
            let reaped_id = zombie.id;
            drop(zombie);
            klog_info!("THREAD_REAPED id={}", reaped_id);
        }

        let threads = match threads_mut().as_mut() {
            Some(t) => t,
            None => bail!(),
        };

        let mut current = match current_mut().take() {
            Some(t) => t,
            None => bail!(), // not initialized yet
        };

        if current.state == ThreadState::Running {
            current.state = ThreadState::Ready;
        }
        let old_rsp_slot = &mut current.saved_rsp as *mut u64;

        // Requeue the outgoing thread (unless it exited) before picking
        // the next one, so a single-thread system safely switches back to
        // itself rather than finding an empty queue. An exited thread is
        // stashed as the zombie for the NEXT tick to reap (see above) —
        // NOT dropped here, which would silently no-op instead of freeing
        // anything (see ZOMBIE's doc comment for why).
        let exited = current.state == ThreadState::Exited;
        if exited {
            *zombie_mut() = Some(current);
        } else {
            threads.push_back(current);
        }

        let mut next = match threads.pop_front() {
            Some(t) => t,
            None => {
                // Phase 9 deliverable 3's real, periodic load balancing:
                // this core has nothing of its own left runnable this
                // tick. Rather than sit idle while another core's queue
                // backs up, steal ONE thread off the back of whichever
                // OTHER core currently holds the most — "even a simple
                // periodic rebalance is acceptable" (docs/ROADMAP.md §5)
                // is exactly this: it runs on every tick that would
                // otherwise go idle, not on a separate timer, and moves
                // at most one thread per steal (no thundering-herd
                // migration). Real cross-core reach: `threads_mut_for`
                // indexes another core's OWN run queue directly, safe
                // here because this whole function already runs under
                // the kernel-wide lock (critical.rs) that now genuinely
                // excludes every other core, not just this one's own
                // interrupts.
                let me = crate::smp::current_cpu_index();
                let mut best_cpu = usize::MAX;
                let mut best_len = 0usize;
                for cpu in 0..crate::smp::MAX_CPUS {
                    if cpu == me {
                        continue;
                    }
                    if let Some(q) = threads_mut_for(cpu).as_ref() {
                        if q.len() > best_len {
                            best_len = q.len();
                            best_cpu = cpu;
                        }
                    }
                }
                if best_cpu != usize::MAX {
                    if let Some(stolen) = threads_mut_for(best_cpu).as_mut().unwrap().pop_back() {
                        klog_info!("SMP_WORK_STOLEN thief_cpu={} victim_cpu={} tid={}", me, best_cpu, stolen.id);
                        stolen
                    } else {
                        bail!(); // lost the race to another concurrent steal attempt on the same tick -- nothing left there either, bail cleanly
                    }
                } else {
                    // Genuinely nothing runnable anywhere (shouldn't
                    // happen once every core has at least its own idle
                    // thread) — put back what we can and bail.
                    bail!();
                }
            }
        };
        next.state = ThreadState::Running;
        let new_rsp = next.saved_rsp;
        let new_address_space = next.address_space;
        // Real root-cause fix (the actual bug behind the multi-process
        // scheduling investigation in critical.rs/PROGRESS.md, found
        // after the pmm/heap/THREADS races above were fixed and the
        // crash still reproduced): TSS.RSP0 -- the kernel stack the CPU
        // switches to automatically on any ring3->ring0 transition
        // (interrupt/exception/syscall) -- is ONE GLOBAL CPU-visible
        // field, but every ring-3 driver thread's own setup code
        // (user_driver.rs, init.rs) called gdt::set_kernel_stack /
        // syscall::set_kernel_stack exactly ONCE, for ITSELF, right
        // before its own first ring-3 entry. That was correct as long as
        // only ONE ring-3 thread was ever alive at a time (every run in
        // this kernel's history before this session's driver work) --
        // whichever thread set it last simply owned it uncontested. With
        // MULTIPLE real ring-3 threads now alive concurrently (init,
        // serial_driver, framebuffer_driver), whichever one's setup code
        // ran LAST overwrote the other two's claim on TSS.RSP0 -- so the
        // NEXT timer interrupt landing while a DIFFERENT one of those
        // threads is actually executing in ring 3 pushes its exception
        // frame onto the WRONG thread's kernel stack, corrupting
        // whatever that stack was actually being used for (that
        // thread's own suspended state, or -- if it happened to be
        // between allocations -- unrelated heap/page-table data the
        // stack pointer no longer legitimately owned). gdt.rs's own doc
        // comment on set_kernel_stack already named this precise gap:
        // "Not yet wired into the scheduler per-thread ... for when
        // ring-3 threads become first-class scheduled entities" -- they
        // just did. Fixed by setting BOTH here, unconditionally, on
        // EVERY switch, to the INCOMING thread's own kernel stack --
        // two MOV-class writes are cheap enough not to bother skipping
        // even when unchanged (unlike the CR3 write below, a full TLB
        // flush, which stays conditional). Thread 0's zero-length
        // `_stack` (kernel_main's own bootstrap context, never entering
        // ring 3) computes a dangling-but-harmless pointer here -- TSS.RSP0
        // is only ever CONSULTED by the CPU on a ring3->ring0 transition,
        // and thread 0 is always already ring 0, so this value is simply
        // never read while thread 0 is the one running.
        let new_kernel_stack_top = next._stack.as_ptr() as u64 + next._stack.len() as u64;
        crate::gdt::set_kernel_stack(new_kernel_stack_top);
        crate::syscall::set_kernel_stack(new_kernel_stack_top);
        // Same real bug class, same fix, applied to the IOPB (see
        // gdt.rs::set_iopb's own doc comment for the full story): the
        // INCOMING thread's own I/O permission bitmap must be reloaded
        // into the one live TSS on every switch, or a port ever granted
        // to some OTHER thread stays visible to this one.
        crate::gdt::set_iopb(&next.iopb);
        *current_mut() = Some(next);

        // CR3 is switched INSIDE switch_to now, not here — see that
        // function's own doc comment for the real double-fault bug this
        // fixes (switching CR3 from out here, while still running on the
        // OUTGOING thread's own stack, unmapped that very stack out from
        // under itself whenever thread 0's special low-half boot stack
        // was on either side of the switch). switch_to still only writes
        // CR3 when it's actually changing (checked in the asm itself),
        // so kernel-thread-to-kernel-thread switches stay free of the
        // TLB-flush overhead exactly as before.

        // old_rsp_slot is a raw pointer into the outgoing thread's boxed
        // Thread struct — stable regardless of the VecDeque itself
        // reallocating (only the Box handle moves, never the heap-
        // allocated Thread it points to). See THREADS's doc comment.
        //
        // Real, necessary, explicit release RIGHT HERE, before the
        // switch -- see `critical.rs::acquire`'s own doc comment for
        // the full deadlock this fixes. `switch_to` never "returns" to
        // this call site in the normal sense for the outgoing thread,
        // so this is the ONLY point that can correctly release the
        // kernel lock on its behalf; `caller_flags` is deliberately NOT
        // restored here (`switch_to`'s own unconditional `sti` already
        // covers whichever thread ends up resumed, the correct
        // behavior regardless of the original caller's own flags).
        crate::critical::release();
        switch_to(old_rsp_slot, new_rsp, new_address_space);
    }
}

/// Real bug found and fixed alongside `spawn_in`'s (same investigation,
/// see `critical.rs`): this runs from ordinary thread context (the tail
/// of every thread's own execution, via `thread_trampoline`), and the
/// take-then-reassign on `CURRENT` below is not a single atomic step. A
/// preemption landing between them would leave `CURRENT` transiently
/// `None`, and `schedule()` firing in that exact window would see
/// "nothing to schedule" and silently no-op that tick instead of
/// switching away from the exiting thread — wrapped now so this
/// mutation, like every other one in this file, can't be interrupted
/// mid-way.
fn exit_current() -> ! {
    crate::critical::without_interrupts(|| unsafe {
        if let Some(mut t) = current_mut().take() {
            t.state = ThreadState::Exited;
            *current_mut() = Some(t);
        }
    });
    loop {
        unsafe { asm!("sti; hlt", options(nomem, nostack)) };
    }
}

/// Kills the CURRENTLY RUNNING thread (marks it `Exited`, permanently
/// removing it from the round-robin via the same deferred-zombie-reap
/// path every normal thread exit already uses) and switches away
/// IMMEDIATELY, without waiting for the next external timer tick.
///
/// Real Phase 1 exit-criterion this closes, unmet since Phase 1 and
/// deferred through Phase 2 (`docs/ROADMAP.md` §5 — "a user-space fault
/// terminates only that process while the system continues"; idt.rs's
/// own doc comment for its fault-handler macros literally said "Phase 1+
/// scope, once processes exist to kill" — real ring-3 processes now
/// exist): every unhandled CPU exception used to halt the WHOLE kernel
/// regardless of which privilege level faulted. `idt.rs`'s handlers now
/// call this instead of halting, for any fault whose `InterruptStackFrame`
/// shows CS's RPL was 3 (ring 3) — a kernel-mode fault still halts
/// unconditionally, since killing "the current thread" when that thread
/// IS the kernel acting on everyone's behalf would be actively dangerous,
/// not a recovery.
///
/// Diverges in the normal case (there's always at least thread 0, the
/// kernel's own idle loop, to switch to) — the only way this function
/// visibly "returns" is the degenerate case where NOTHING is runnable
/// (shouldn't happen once thread 0 exists), which callers must still
/// treat as fatal.
pub fn kill_current_and_reschedule() {
    crate::critical::without_interrupts(|| unsafe {
        if let Some(t) = current_mut().as_mut() {
            t.state = ThreadState::Exited;
        }
    });
    schedule();
}

/// One-time setup: makes the calling context (kernel_main, post-Phase-0,
/// running on the BSP) "thread 0" so `schedule()` has something valid
/// to save into on the very first timer tick. `current_mut()` resolves
/// to the BSP's OWN slot (`smp::current_cpu_index()` == 0 at this point
/// in boot), so this only ever touches core 0's state.
pub fn init_as_current_thread() {
    crate::critical::without_interrupts(|| unsafe { init_as_current_thread_locked() });
}

/// Phase 9 deliverable 3: same idea as `init_as_current_thread`, for an
/// AP claiming ITS OWN idle "thread 0" once it's running (`smp.rs::
/// ap_entry`, after `gdt::init_for_cpu`/`idt::load_current_cpu` have
/// already run) — every real core needs a valid `CURRENTS[cpu]` before
/// its own first timer tick can call `schedule()`, exactly the same
/// bootstrap need the BSP already had, just per-core now instead of
/// global.
pub fn init_as_current_thread_for_cpu() {
    crate::critical::without_interrupts(|| unsafe { init_as_current_thread_locked() });
}

unsafe fn init_as_current_thread_locked() {
    unsafe {
        // Real, unique id per core's own idle thread -- a hardcoded 0
        // was correct back when there was only ever ONE such thread
        // (the BSP's); with one of these per real core now, a shared
        // hardcoded id would collide across cores in audit attribution
        // and introspection (`thread::snapshot`), silently conflating
        // unrelated cores' own kernel-idle context under one identity.
        let tid = NEXT_TID;
        NEXT_TID += 1;
        let stack = alloc::vec![0u8; 0].into_boxed_slice(); // this core's real boot/entry stack isn't ours to own
        *current_mut() = Some(Box::new(Thread {
            id: tid,
            state: ThreadState::Running,
            saved_rsp: 0, // never read until this thread is switched OUT of, which fills it in
            _stack: stack,
            address_space: crate::vmm::kernel_pml4_phys(),
            cap_table: crate::capability::CapabilityTable::new(),
            iopb: [0xFFu8; crate::gdt::IOPB_BYTES],
        }));
    }
}

/// Top address of the CURRENTLY RUNNING thread's own kernel stack — for
/// `gdt::set_kernel_stack`/`syscall::set_kernel_stack` to use instead of a
/// separate scratch allocation. Real bug this session found by tracing
/// through what happens when a syscall blocks and gets preempted: a
/// separate scratch stack for syscall/interrupt re-entry works fine right
/// up until something running ON it gets preempted (schedule() saves
/// whatever RSP is current into the THREAD's own saved_rsp — but that RSP
/// pointed into the scratch buffer, not this thread's real stack, so a
/// LATER unrelated thread reusing that same scratch region for ITS OWN
/// syscall would silently corrupt the first thread's still-suspended
/// state). Using the thread's own already-allocated kernel stack instead
/// means a mid-syscall preemption is just an ordinary, correctly-tracked
/// context switch — nothing else can ever collide with it, because it's
/// the same stack that thread already owns for its whole lifetime.
pub fn current_kernel_stack_top() -> u64 {
    crate::critical::without_interrupts(|| unsafe {
        current_mut()
            .as_ref()
            .map(|t| t._stack.as_ptr() as u64 + t._stack.len() as u64)
            .unwrap_or(0)
    })
}

/// Updates the CURRENTLY RUNNING thread's tracked `address_space` field.
/// Real bug this session found: a thread spawned via plain `spawn()`
/// (shared kernel PML4) that LATER calls `vmm::switch_address_space`
/// manually (as the ring-3 demo does, to enter a freshly-created process
/// address space at runtime) changes CR3 WITHOUT `thread.rs` ever finding
/// out — the Thread struct's own `address_space` field stays stale at
/// whatever it was spawned with. The instant this thread is preempted and
/// later resumed, `schedule()` switches CR3 back to that STALE tracked
/// value (the kernel's, not the process's), silently pulling the rug out
/// from under whatever address space the thread thought it was still
/// running in — manifested as a page fault on a page that had been
/// present moments before, because it genuinely was no longer the active
/// address space. Any code that manually switches its own address space
/// must call this right after, or preemption can and will undo it.
pub fn set_current_address_space(pml4_phys: u64) {
    crate::critical::without_interrupts(|| unsafe {
        if let Some(t) = current_mut().as_mut() {
            t.address_space = pml4_phys;
        }
    });
}

pub fn current_id() -> ThreadId {
    crate::critical::without_interrupts(|| unsafe { current_mut().as_ref().map(|t| t.id).unwrap_or(0) })
}

/// Real, typed snapshot of every live thread ACROSS EVERY REAL CORE —
/// `(id, state, is_user)`, `is_user` meaning this thread runs in its own
/// process address space rather than the shared kernel one. Phase 5's
/// introspection API (`introspect.rs`) builds its `ThreadInfo` structs
/// from exactly this, not from any text-formatted log line — the whole
/// point of "typed interfaces, no text scraping" (`docs/ROADMAP.md`'s
/// Phase 5 exit criteria) is that this function returns real struct
/// data, the same data the scheduler itself operates on, not a
/// re-parsed rendering of it. Takes the same `critical::without_interrupts`
/// lock every other RUN_QUEUES/CURRENTS accessor in this file does — a
/// caller (a syscall handler) walking this while `schedule()` is
/// mid-mutation on ANY core would be the exact same TOCTOU class
/// already fixed everywhere else here.
///
/// Phase 9 deliverable 3: walks EVERY core's own `CURRENTS`/`RUN_QUEUES`
/// slot (`threads_mut_for`/`current_ref_for`, not the calling core's own
/// `current_mut`/`threads_mut`) — a single-core `snapshot()` would
/// silently under-report the moment more than one core has real threads
/// running, exactly the kind of introspection gap Phase 5's own "typed,
/// not text-scraped" discipline exists to catch.
pub fn snapshot() -> alloc::vec::Vec<(ThreadId, ThreadState, bool)> {
    crate::critical::without_interrupts(|| unsafe {
        let kernel_pml4 = crate::vmm::kernel_pml4_phys();
        let mut out = alloc::vec::Vec::new();
        for cpu in 0..crate::smp::MAX_CPUS {
            if let Some(t) = current_ref_for(cpu).as_ref() {
                out.push((t.id, ThreadState::Running, t.address_space != kernel_pml4));
            }
            if let Some(threads) = threads_mut_for(cpu).as_ref() {
                for t in threads.iter() {
                    out.push((t.id, t.state, t.address_space != kernel_pml4));
                }
            }
        }
        out
    })
}
