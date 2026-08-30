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

const KERNEL_STACK_SIZE: usize = 64 * 1024;

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
}

static mut NEXT_TID: ThreadId = 1;
// Box<Thread>, not Thread, in both places: `schedule()` takes a raw
// pointer into the current thread's `saved_rsp` field BEFORE requeuing it,
// and a `VecDeque<Thread>` can move existing elements on reallocation,
// which would silently invalidate that pointer. A `VecDeque<Box<Thread>>`
// only ever moves the (small, stable-pointee) Box handle on reallocation —
// the Thread itself, and any raw pointer into its fields, stays put on the
// heap regardless. Found by review, not by a crash — worth fixing before
// it became one.
static mut THREADS: Option<VecDeque<Box<Thread>>> = None;
static mut CURRENT: Option<Box<Thread>> = None;

// `&raw mut` + deref, not `&mut THREADS`/`&mut CURRENT` directly — the
// compiler's own suggested fix for the static_mut_refs lint everywhere
// else in this codebase (gdt.rs, idt.rs, pmm.rs). Centralized here since
// thread.rs's scheduler touches both statics from several functions.
#[allow(static_mut_refs)]
unsafe fn threads_mut() -> &'static mut Option<VecDeque<Box<Thread>>> {
    &mut *&raw mut THREADS
}
#[allow(static_mut_refs)]
unsafe fn current_mut() -> &'static mut Option<Box<Thread>> {
    &mut *&raw mut CURRENT
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
        });

        if threads_mut().is_none() {
            *threads_mut() = Some(VecDeque::new());
        }
        threads_mut().as_mut().unwrap().push_back(thread);
        tid
    }
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
static mut ZOMBIE: Option<Box<Thread>> = None;
#[allow(static_mut_refs)]
unsafe fn zombie_mut() -> &'static mut Option<Box<Thread>> {
    &mut *&raw mut ZOMBIE
}

/// Called from `idt.rs::h_timer` on every tick. Picks the next Ready
/// thread round-robin and switches to it; a no-op if there's nothing else
/// runnable yet (Phase 1's early state, before more than one thread
/// exists) or scheduling hasn't been initialized.
///
/// Wrapped in `without_interrupts` defensively — this already only ever
/// runs with hardware IF=0 (interrupt-gate entry), so the wrapper is a
/// documented no-op here today, not a fix in itself. It exists so the
/// invariant ("nothing touches THREADS/CURRENT/ZOMBIE without interrupts
/// disabled") is enforced uniformly and stays true even if this function
/// is ever called from a differently-configured gate later.
pub fn schedule() {
    crate::critical::without_interrupts(|| unsafe { schedule_locked() });
}

unsafe fn schedule_locked() {
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
            None => return,
        };

        let mut current = match current_mut().take() {
            Some(t) => t,
            None => return, // not initialized yet
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
                // Nothing runnable (shouldn't happen once the idle thread
                // exists) — put back what we can and bail.
                return;
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

/// One-time setup: makes the calling context (kernel_main, post-Phase-0)
/// "thread 0" so `schedule()` has something valid to save into on the
/// very first timer tick.
pub fn init_as_current_thread() {
    unsafe {
        let stack = alloc::vec![0u8; 0].into_boxed_slice(); // kernel_main's real stack isn't ours to own
        *current_mut() = Some(Box::new(Thread {
            id: 0,
            state: ThreadState::Running,
            saved_rsp: 0, // never read until this thread is switched OUT of, which fills it in
            _stack: stack,
            address_space: crate::vmm::kernel_pml4_phys(),
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
