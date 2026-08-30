//! SYSCALL/SYSRET entry path. Phase 1 item — the fast user->kernel->user
//! transition (vs. `int 0x80`-style, which goes through the full IDT gate
//! machinery). Nothing in this kernel has ever handled a syscall before
//! this.
//!
//! Single-core simplification, stated plainly: real multi-core kernels
//! (Linux included) use `swapgs` + `IA32_GS_BASE` to reach a per-CPU
//! kernel-stack pointer from inside the entry stub, because with multiple
//! cores each needs its OWN kernel stack reachable without any shared
//! mutable state. This kernel has no SMP yet (a Phase 2+ concern at the
//! earliest), so a single plain static holding "the current thread's
//! kernel stack" is completely correct here and meaningfully simpler.
//! Revisit with real per-CPU state before this kernel ever runs on more
//! than one core — that's a real, load-bearing constraint on this design,
//! not a corner cut for convenience.
//!
//! GDT ordering note: `gdt.rs` deliberately placed user data (0x28) right
//! before user code (0x30) specifically so `STAR`'s arithmetic works out —
//! SYSCALL computes kernel SS as `STAR[32:47] + 8`; SYSRET computes user SS
//! as `STAR[48:63] + 8` and user CS as `STAR[48:63] + 16`. `STAR[48:63]`
//! itself (0x20) is never loaded as a real descriptor — it's purely an
//! arithmetic base — so it not being a meaningful selector on its own
//! (it lands on the TSS's second 8-byte slot) is fine.

use crate::klog_info;

const IA32_EFER: u32 = 0xC000_0080;
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;
const EFER_SCE: u64 = 1 << 0; // System Call Extensions enable

static mut KERNEL_RSP: u64 = 0;
static mut USER_RSP_SCRATCH: u64 = 0;

unsafe fn rdmsr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    core::arch::asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack, preserves_flags));
    ((high as u64) << 32) | (low as u64)
}

unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!("wrmsr", in("ecx") msr, in("eax") low, in("edx") high, options(nomem, nostack, preserves_flags));
}

/// Sets the kernel stack `syscall_entry` switches to — analogous to
/// `gdt::set_kernel_stack` (TSS.RSP0), same "not yet wired per-thread into
/// the scheduler" gap noted there applies here too.
pub fn set_kernel_stack(rsp: u64) {
    unsafe { KERNEL_RSP = rsp };
}

/// Dispatches one syscall. `num` is RAX at entry; `a0`/`a1` are the first
/// two Linux-convention argument registers (RDI, RSI) — only as many as
/// this Phase 1 proof-of-concept's demo syscalls need, not a complete ABI
/// yet. Returns the value placed back into RAX for the caller.
///
/// NO user-pointer validation exists yet for syscalls that would take one
/// — none of the demo syscalls below dereference a user-supplied address,
/// so there's nothing to validate yet. `docs/ROADMAP.md`'s "syscalls
/// validate all user pointers" requirement stays open until a syscall
/// that actually takes a pointer argument exists — see PHASE1_PROGRESS.md.
#[no_mangle]
extern "C" fn syscall_dispatch(num: u64, a0: u64, _a1: u64) -> u64 {
    match num {
        1 => {
            klog_info!("SYSCALL_LOG value=0x{:x}", a0);
            0
        }
        _ => {
            klog_info!("SYSCALL_UNKNOWN num={}", num);
            u64::MAX
        }
    }
}

/// The SYSCALL entry point (loaded into IA32_LSTAR). Naked: this runs with
/// CS/SS already switched to kernel selectors (per STAR) but RSP is STILL
/// the user's — SYSCALL does not switch stacks automatically the way an
/// interrupt-gate does via TSS.RSP0. RCX holds the user return RIP, R11
/// the user RFLAGS (both CPU-saved by SYSCALL itself); both MUST survive
/// untouched until SYSRETQ, which is exactly why they're saved/restored
/// here rather than treated as scratch.
#[unsafe(naked)]
extern "C" fn syscall_entry() {
    core::arch::naked_asm!(
        "mov [{user_rsp}], rsp",     // stash the user RSP
        "mov rsp, [{kernel_rsp}]",   // switch onto the kernel stack
        "push rcx",                  // user RIP (SYSCALL-saved) — must survive to sysretq
        "push r11",                  // user RFLAGS (SYSCALL-saved) — same
        // Syscall args arrive in rdi/rsi/rdx/r10/r8/r9 (Linux convention,
        // r10 not rcx — rcx is consumed by SYSCALL itself); num is in rax.
        // syscall_dispatch(num=rax, a0=rdi, a1=rsi) via the C calling
        // convention: rdi<-rax, rsi<-rdi(orig), rdx<-rsi(orig).
        "mov r12, rdi",              // save real a0 (was in rdi) past the reshuffle below
        "mov r13, rsi",              // save real a1
        "mov rdi, rax",              // dispatch arg0 = syscall number
        "mov rsi, r12",              // dispatch arg1 = a0
        "mov rdx, r13",              // dispatch arg2 = a1
        "call {dispatch}",
        // return value already in rax, exactly where sysretq's caller expects it
        "pop r11",
        "pop rcx",
        "mov rsp, [{user_rsp}]",     // back onto the user's own stack
        "sysretq",
        user_rsp = sym USER_RSP_SCRATCH,
        kernel_rsp = sym KERNEL_RSP,
        dispatch = sym syscall_dispatch,
    );
}

pub fn init() {
    unsafe {
        let efer = rdmsr(IA32_EFER);
        wrmsr(IA32_EFER, efer | EFER_SCE);

        // STAR[32:47] = kernel CS base (0x08); SYSCALL sets CS=that,
        // SS=that+8 (0x10, our kernel data — matches gdt.rs).
        // STAR[48:63] = 0x20, purely arithmetic for SYSRET: user
        // SS = 0x20+8 = 0x28, user CS = 0x20+16 = 0x30 (both matching
        // gdt.rs's USER_DATA_SELECTOR/USER_CODE_SELECTOR base, RPL bits
        // added separately by the CPU per SYSRET's own rules).
        let star: u64 = (0x0020u64 << 48) | (0x0008u64 << 32);
        wrmsr(IA32_STAR, star);

        wrmsr(IA32_LSTAR, syscall_entry as *const () as u64);

        // RFLAGS bits cleared on entry — mask IF so the entry stub itself
        // (before the stack switch above is even reachable in the
        // interrupt-safety sense) can't be interrupted mid-transition.
        wrmsr(IA32_FMASK, 0x200);
    }
    klog_info!("SYSCALL/SYSRET initialized (single-core kernel-stack model)");
}
