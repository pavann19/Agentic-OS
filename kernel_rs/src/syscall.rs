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
use crate::{capability, driver, ipc, thread, vmm};

// Phase 2's syscall-surface proof: a dedicated table/capability reachable
// from ring 3 via syscall number 2, independent of the kernel-thread-level
// capability demo in main.rs (deliberately not reusing that demo's state —
// it revokes its own capability partway through, which would make this
// syscall path's success order-dependent on unrelated demo internals).
static mut RING3_IPC_TABLE: Option<capability::CapabilityTable> = None;
static RING3_SEND_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// One-time setup so syscall number 2 (below) has a real capability to
/// invoke. Called once from main.rs before entering ring 3.
pub fn init_ring3_ipc_demo() -> capability::CapId {
    unsafe {
        RING3_IPC_TABLE = Some(capability::CapabilityTable::new());
        let table = (&mut *&raw mut RING3_IPC_TABLE).as_mut().unwrap();
        let cap = ipc::create_endpoint(
            table,
            capability::Rights::SEND.union(capability::Rights::RECEIVE),
        );
        RING3_SEND_CAP.store(cap, core::sync::atomic::Ordering::SeqCst);
        cap
    }
}

pub fn ring3_ipc_table() -> &'static capability::CapabilityTable {
    unsafe { (*(&raw const RING3_IPC_TABLE)).as_ref().unwrap() }
}

pub fn ring3_send_cap() -> capability::CapId {
    RING3_SEND_CAP.load(core::sync::atomic::Ordering::SeqCst)
}

// Phase 3's init->service-manager handoff: a second, separate capability
// table/endpoint from the Phase 2 demo above — deliberately not reusing
// RING3_IPC_TABLE, since that demo's capability gets revoked partway
// through its own run and this path must stay correct independent of
// that unrelated demo's internal state (same reasoning noted for
// RING3_IPC_TABLE itself).
static mut INIT_SVC_TABLE: Option<capability::CapabilityTable> = None;
static INIT_SVC_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// One-time setup so syscall number 3 (below) has a real capability to
/// invoke. Called once from main.rs before spawning the init process.
pub fn init_service_ipc() -> capability::CapId {
    unsafe {
        INIT_SVC_TABLE = Some(capability::CapabilityTable::new());
        let table = (&mut *&raw mut INIT_SVC_TABLE).as_mut().unwrap();
        let cap = ipc::create_endpoint(
            table,
            capability::Rights::SEND.union(capability::Rights::RECEIVE),
        );
        INIT_SVC_CAP.store(cap, core::sync::atomic::Ordering::SeqCst);
        cap
    }
}

pub fn init_svc_table() -> &'static capability::CapabilityTable {
    unsafe { (*(&raw const INIT_SVC_TABLE)).as_ref().unwrap() }
}

pub fn init_svc_cap() -> capability::CapId {
    INIT_SVC_CAP.load(core::sync::atomic::Ordering::SeqCst)
}

// Phase 3's framebuffer driver readiness signal: a third, dedicated
// capability table/endpoint, same reasoning as INIT_SVC_TABLE above --
// each syscall-reachable operation gets its own capability, never a
// shared ambient one.
static mut FB_READY_TABLE: Option<capability::CapabilityTable> = None;
static FB_READY_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// One-time setup so syscall number 4 (below) has a real capability to
/// invoke. Called once from user_driver.rs before spawning the
/// framebuffer driver.
pub fn init_fb_ready_ipc() -> capability::CapId {
    unsafe {
        FB_READY_TABLE = Some(capability::CapabilityTable::new());
        let table = (&mut *&raw mut FB_READY_TABLE).as_mut().unwrap();
        let cap = ipc::create_endpoint(
            table,
            capability::Rights::SEND.union(capability::Rights::RECEIVE),
        );
        FB_READY_CAP.store(cap, core::sync::atomic::Ordering::SeqCst);
        cap
    }
}

pub fn fb_ready_table() -> &'static capability::CapabilityTable {
    unsafe { (*(&raw const FB_READY_TABLE)).as_ref().unwrap() }
}

pub fn fb_ready_cap() -> capability::CapId {
    FB_READY_CAP.load(core::sync::atomic::Ordering::SeqCst)
}

// Phase 3's PS/2 keyboard driver: a fourth dedicated capability table,
// same pattern as the three above -- but holding an `InterruptLine`
// capability (`driver.rs`), not an IPC endpoint. Syscalls 5/6 below let a
// REAL ring-3 process block-wait on it and acknowledge it repeatedly
// (unlike syscalls 2/3/4's one-shot sends) -- the first syscall pair in
// this kernel that mediates an ONGOING driver operation rather than a
// single handoff.
static mut KBD_TABLE: Option<capability::CapabilityTable> = None;
static KBD_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// One-time setup so syscalls 5/6 (below) have a real `InterruptLine`
/// capability for `pic::KEYBOARD_VECTOR` to operate on. Called once from
/// user_driver.rs before spawning the keyboard driver process --
/// `driver::create_interrupt_capability` also registers the vector with
/// `interrupt_forward`, the same real mechanism `idt.rs::h_keyboard`
/// notifies.
pub fn init_kbd_capability() {
    unsafe {
        KBD_TABLE = Some(capability::CapabilityTable::new());
        let table = (&mut *&raw mut KBD_TABLE).as_mut().unwrap();
        let cap = driver::create_interrupt_capability(
            table,
            crate::pic::KEYBOARD_VECTOR,
            capability::Rights::WAIT,
        );
        KBD_CAP.store(cap, core::sync::atomic::Ordering::SeqCst);
    }
}

fn kbd_table() -> &'static capability::CapabilityTable {
    unsafe { (*(&raw const KBD_TABLE)).as_ref().unwrap() }
}

fn kbd_cap() -> capability::CapId {
    KBD_CAP.load(core::sync::atomic::Ordering::SeqCst)
}

// Phase 5's agent-facing syscalls (7, 9) deliberately have NO dedicated
// static table here, unlike syscalls 2-6 above: those all serve exactly
// one fixed process each, so a single kernel-wide static table/capability
// pair is correct for them. Syscalls 7/9 below serve MULTIPLE, independent
// agent processes (`agent.rs`), each with its OWN capability set — see
// `thread::Thread::cap_table` and `thread::spawn_with_capabilities`/
// `resolve_current_capability`, the real per-process mechanism this
// needed instead. By convention (see `agent.rs::spawn_agent_demo`'s
// grant order), an authorized agent's own cap_table holds the
// introspection capability at slot 0 and the audit-query capability at
// slot 1, if granted at all.
const AGENT_INTROSPECT_CAP: capability::CapId = 0;
const AGENT_AUDIT_QUERY_CAP: capability::CapId = 1;

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
/// `gdt::set_kernel_stack` (TSS.RSP0). NOW wired into the scheduler
/// per-thread too (`thread.rs::schedule_locked` calls this alongside
/// `gdt::set_kernel_stack` on every switch) — same real bug, same fix,
/// see `gdt::set_kernel_stack`'s doc comment for the full story.
pub fn set_kernel_stack(rsp: u64) {
    unsafe { KERNEL_RSP = rsp };
}

/// Dispatches one syscall. `num` is RAX at entry; `a0`/`a1` are the first
/// two Linux-convention argument registers (RDI, RSI) — only as many as
/// this kernel's syscalls need so far, not a complete ABI yet. Returns
/// the value placed back into RAX for the caller.
///
/// Real user-pointer validation now exists (Phase 5, syscall 7 below is
/// the first to dereference a caller-supplied address) —
/// `vmm::validate_user_buffer_writable` walks the CALLING process's own
/// page tables (`vmm::current_cr3()`, never a trusted kernel one) and
/// requires PRESENT+WRITABLE+USER on every page in range before the
/// kernel ever writes through it. The long-standing "no user-pointer
/// validation exists yet" note (open since Phase 1, see PROGRESS.md) is
/// closed for the syscall that actually needed it; syscalls 1-6 above
/// still take no pointer arguments, so there's nothing there to validate.
#[no_mangle]
extern "C" fn syscall_dispatch(num: u64, a0: u64, a1: u64) -> u64 {
    match num {
        1 => {
            klog_info!("SYSCALL_LOG value=0x{:x}", a0);
            0
        }
        2 => {
            // Phase 2's syscall-surface proof: a ring-3 process invoking a
            // REAL capability-gated kernel operation (IPC send) through
            // the syscall path, not just a kernel thread calling ipc::send
            // directly. `a0` is the message payload. Capability
            // enforcement is not bypassed for syscalls — this still goes
            // through the exact same `CapabilityTable::resolve` every
            // other caller does; a ring-3 process gets nothing syscalls
            // don't explicitly grant it.
            let table = ring3_ipc_table();
            let cap = RING3_SEND_CAP.load(core::sync::atomic::Ordering::SeqCst);
            let mut msg = ipc::Message::default();
            msg.data[0] = a0;
            match ipc::send(table, cap, msg) {
                Ok(()) => {
                    klog_info!("SYSCALL_IPC_SEND delivered value=0x{:x}", a0);
                    0
                }
                Err(_) => u64::MAX,
            }
        }
        3 => {
            // Phase 3's init->service-manager handoff: the real `init`
            // process (init.rs, unconditional part of normal boot, not
            // feature-gated like the demo_ring3 proof) tells the service
            // manager it's ready via this real capability-gated IPC send
            // — same enforcement discipline as syscall 2, a distinct
            // capability table so this path's correctness doesn't depend
            // on any other demo's internal state.
            let table = init_svc_table();
            let cap = INIT_SVC_CAP.load(core::sync::atomic::Ordering::SeqCst);
            let mut msg = ipc::Message::default();
            msg.data[0] = a0;
            match ipc::send(table, cap, msg) {
                Ok(()) => {
                    klog_info!("SYSCALL_SVC_START token=0x{:x}", a0);
                    0
                }
                Err(_) => u64::MAX,
            }
        }
        4 => {
            // Phase 3's framebuffer driver readiness signal: the real
            // ELF-loaded framebuffer driver (user_driver.rs) tells the
            // kernel-side verify thread it has finished writing its test
            // pattern into the real GOP framebuffer -- same real
            // capability-gated IPC discipline as syscalls 2/3.
            let table = fb_ready_table();
            let cap = FB_READY_CAP.load(core::sync::atomic::Ordering::SeqCst);
            let mut msg = ipc::Message::default();
            msg.data[0] = a0;
            match ipc::send(table, cap, msg) {
                Ok(()) => {
                    klog_info!("SYSCALL_FB_READY token=0x{:x}", a0);
                    0
                }
                Err(_) => u64::MAX,
            }
        }
        5 => {
            // Phase 3's PS/2 keyboard driver: blocks the calling ring-3
            // thread (via driver::wait_interrupt -> the same real
            // interrupt_forward rendezvous idt.rs::h_keyboard notifies)
            // until IRQ1 fires. Capability-gated identically to every
            // other driver.rs operation -- Rights::WAIT is checked, not
            // bypassed for being reached via syscall.
            match driver::wait_interrupt(kbd_table(), kbd_cap()) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        6 => {
            // Distinct acknowledge step, same real/audited-separately
            // discipline interrupt_forward.rs documents for the
            // kernel-thread-level demo -- a real driver's "I saw the IRQ"
            // and "I finished handling it" are genuinely different
            // moments here too.
            match driver::ack_interrupt(kbd_table(), kbd_cap()) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        7 => {
            // Phase 5's structured introspection API
            // (`docs/ROADMAP.md` §5 Phase 5, deliverable 2): a0 = the
            // calling process's own buffer address, a1 = its capacity in
            // ThreadInfo-sized entries. Capability-gated on the CALLING
            // thread's OWN cap_table (`thread::resolve_current_capability`
            // — see that function's doc for why a per-process table,
            // not a shared global, is what makes two agent processes
            // with different grants genuinely behave differently here) —
            // enforced by the kernel's own check, not the agent's
            // cooperation, exactly Phase 5's second exit criterion.
            match thread::resolve_current_capability(AGENT_INTROSPECT_CAP, capability::Rights::INTROSPECT) {
                Ok(_) => {
                    let max_entries = a1 as usize;
                    let entries = crate::introspect::snapshot_threads(max_entries);
                    let total_bytes = entries.len() as u64 * crate::introspect::THREAD_INFO_SIZE;
                    let pml4 = vmm::current_cr3();
                    if total_bytes == 0
                        || !unsafe { vmm::validate_user_buffer_writable(pml4, a0, total_bytes) }
                    {
                        klog_info!("SYSCALL_INTROSPECT_BAD_BUFFER");
                        return u64::MAX;
                    }
                    for (i, info) in entries.iter().enumerate() {
                        let bytes = crate::introspect::thread_info_bytes(info);
                        let dst = a0 + (i as u64) * crate::introspect::THREAD_INFO_SIZE;
                        unsafe { vmm::write_user_bytes(pml4, dst, &bytes) };
                    }
                    entries.len() as u64
                }
                Err(_) => u64::MAX,
            }
        }
        8 => {
            // Phase 5's tool/intent surface (`docs/ROADMAP.md` §5 Phase
            // 5, deliverable 3): real, typed, DISCOVERABLE catalog —
            // deliberately NOT capability-gated, matching "discoverable"
            // (an agent can see what operations exist and what they'd
            // require without yet holding anything; USING what it finds
            // still goes through every capability check that operation
            // already has). a0 = buffer, a1 = capacity in
            // ToolDescriptor-sized entries.
            let catalog = crate::tools::catalog();
            let max_entries = (a1 as usize).min(catalog.len());
            let total_bytes = max_entries as u64 * crate::tools::TOOL_DESCRIPTOR_SIZE;
            let pml4 = vmm::current_cr3();
            if total_bytes == 0 || !unsafe { vmm::validate_user_buffer_writable(pml4, a0, total_bytes) } {
                klog_info!("SYSCALL_TOOLS_BAD_BUFFER");
                return u64::MAX;
            }
            for (i, d) in catalog.iter().take(max_entries).enumerate() {
                let bytes = crate::tools::tool_descriptor_bytes(d);
                let dst = a0 + (i as u64) * crate::tools::TOOL_DESCRIPTOR_SIZE;
                unsafe { vmm::write_user_bytes(pml4, dst, &bytes) };
            }
            max_entries as u64
        }
        9 => {
            // Phase 5's capability-scoped audit query API
            // (`docs/ROADMAP.md` §5 Phase 5, deliverable 5): real
            // typed `AuditEntryInfo` records, filtered to exactly the
            // CALLING process's own actor_tid
            // (`audit::records_by_actor`) — never the whole log. a0 =
            // buffer, a1 = capacity. Capability-gated the same way as
            // syscall 7: the caller's OWN cap_table must hold
            // Rights::AUDIT_QUERY.
            match thread::resolve_current_capability(AGENT_AUDIT_QUERY_CAP, capability::Rights::AUDIT_QUERY) {
                Ok(_) => {
                    let max_entries = a1 as usize;
                    let tid = thread::current_id();
                    let entries = crate::introspect::snapshot_audit_for(tid, max_entries);
                    let total_bytes = entries.len() as u64 * crate::introspect::AUDIT_ENTRY_SIZE;
                    let pml4 = vmm::current_cr3();
                    if total_bytes == 0
                        || !unsafe { vmm::validate_user_buffer_writable(pml4, a0, total_bytes) }
                    {
                        // A genuinely empty result (this process caused
                        // nothing auditable yet) is real, not an error —
                        // but there's no buffer write to validate against
                        // when there's nothing to write, so return 0
                        // directly rather than treating it as a bad-buffer
                        // failure.
                        return if entries.is_empty() { 0 } else { u64::MAX };
                    }
                    for (i, e) in entries.iter().enumerate() {
                        let bytes = crate::introspect::audit_entry_bytes(e);
                        let dst = a0 + (i as u64) * crate::introspect::AUDIT_ENTRY_SIZE;
                        unsafe { vmm::write_user_bytes(pml4, dst, &bytes) };
                    }
                    entries.len() as u64
                }
                Err(_) => u64::MAX,
            }
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
        // Real bug this session found: IA32_FMASK clears IF on SYSCALL
        // entry, and dispatch is free to call blocking operations
        // (ipc::send/receive spin-yield via hlt, waiting for another
        // thread to run) — with interrupts still masked, the timer can
        // never fire, schedule() never runs, and a blocking syscall
        // deadlocks the entire machine forever. `sti` here, AFTER the two
        // pushes above are safely on the kernel stack, fixes it — same
        // STI-shadow reasoning as thread.rs's switch_to (the enable
        // doesn't take effect until after the NEXT instruction), so
        // nothing can be preempted mid-push.
        "sti",
        // Real bug found and fixed (root-caused via Phase 4's
        // virtio_blk_driver, the first caller whose code actually kept a
        // value live in a callee-saved register — r13 — across a
        // syscall): r12/r13 were used here as scratch space to reshuffle
        // arguments into dispatch's own calling convention, WITHOUT
        // saving/restoring the caller's original r12/r13 first. SysV C
        // ABI (and every user-space caller's own reasonable assumption)
        // treats r12-r15/rbx/rbp as callee-saved — a syscall silently
        // clobbering two of them broke that contract for every syscall
        // this kernel has ever executed, it just never had an observed
        // symptom before now, because no earlier caller's code needed
        // r12/r13 to still hold anything meaningful after the call.
        // Pushed/popped now, the same real save-then-restore discipline
        // rcx/r11 already get two lines below.
        "push r12",
        "push r13",
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
        "pop r13",
        "pop r12",
        "pop r11",
        "pop rcx",
        // `cli` before swapping onto the user's own stack: right after
        // that swap, RSP holds a user-space address but CS is STILL the
        // kernel selector (sysretq hasn't run yet) — CPL is still 0, so
        // an interrupt landing in that window would push its frame using
        // the CURRENT RSP (no privilege-change stack switch happens,
        // because CPL isn't changing), i.e. onto the USER's stack from
        // kernel context. `cli` closes that window; sysretq itself
        // restores IF from R11 (the user's original RFLAGS, IF=1) the
        // instant it lands back in ring 3, so nothing stays disabled
        // longer than this narrow gap.
        "cli",
        "mov rsp, [{user_rsp}]",     // back onto the user's own stack
        "sysretq",
        user_rsp = sym USER_RSP_SCRATCH,
        kernel_rsp = sym KERNEL_RSP,
        dispatch = sym syscall_dispatch,
    );
}

static SYSCALL_INITIALIZED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Idempotent: init.rs's real init process and the feature-gated
/// demo_ring3 proof can both run in the same build and each call this
/// once before entering ring 3 for the first time. Re-running the MSR
/// writes with the same values would be harmless anyway, but this avoids
/// a confusing duplicate "initialized" log line.
pub fn init() {
    if SYSCALL_INITIALIZED.swap(true, core::sync::atomic::Ordering::SeqCst) {
        return;
    }
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
