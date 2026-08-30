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
        });

        if threads_mut().is_none() {
            *threads_mut() = Some(VecDeque::new());
        }
        threads_mut().as_mut().unwrap().push_back(thread);
        tid
    }
}

/// Raw asm: save callee-saved regs + RSP into `*old_rsp_slot`, load
/// `new_rsp`, restore ITS callee-saved regs, `ret`. Never returns to its
/// direct caller in the normal sense — control resumes wherever the target
/// thread last left off (or thread_trampoline, first time).
#[unsafe(naked)]
unsafe extern "C" fn switch_to(old_rsp_slot: *mut u64, new_rsp: u64) {
    core::arch::naked_asm!(
        "push rbp",
        "push rbx",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",   // *old_rsp_slot = current RSP (after pushes)
        "mov rsp, rsi",     // RSP = new_rsp
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

/// Called from `idt.rs::h_timer` on every tick. Picks the next Ready
/// thread round-robin and switches to it; a no-op if there's nothing else
/// runnable yet (Phase 1's early state, before more than one thread
/// exists) or scheduling hasn't been initialized.
pub fn schedule() {
    unsafe {
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
        // itself rather than finding an empty queue.
        let exited = current.state == ThreadState::Exited;
        if !exited {
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
        *current_mut() = Some(next);

        // old_rsp_slot is a raw pointer into the outgoing thread's boxed
        // Thread struct — stable regardless of the VecDeque itself
        // reallocating (only the Box handle moves, never the heap-
        // allocated Thread it points to). See THREADS's doc comment.
        switch_to(old_rsp_slot, new_rsp);
    }
}

fn exit_current() -> ! {
    unsafe {
        if let Some(mut t) = current_mut().take() {
            t.state = ThreadState::Exited;
            *current_mut() = Some(t);
        }
    }
    loop {
        unsafe { asm!("sti; hlt", options(nomem, nostack)) };
    }
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
        }));
    }
}

pub fn current_id() -> ThreadId {
    unsafe { current_mut().as_ref().map(|t| t.id).unwrap_or(0) }
}
