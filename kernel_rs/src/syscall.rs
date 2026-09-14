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

// Real GUI mouse support: the exact same "one fixed process, one
// kernel-wide static table/capability pair" pattern as KBD_TABLE/
// KBD_CAP right above -- `mouse_driver.rs` is the only process that
// ever holds this InterruptLine capability, same as keyboard_driver.
static mut MOUSE_TABLE: Option<capability::CapabilityTable> = None;
static MOUSE_CAP: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn init_mouse_capability() {
    unsafe {
        MOUSE_TABLE = Some(capability::CapabilityTable::new());
        let table = (&mut *&raw mut MOUSE_TABLE).as_mut().unwrap();
        let cap = driver::create_interrupt_capability(
            table,
            crate::pic::MOUSE_VECTOR,
            capability::Rights::WAIT,
        );
        MOUSE_CAP.store(cap, core::sync::atomic::Ordering::SeqCst);
    }
}

fn mouse_table() -> &'static capability::CapabilityTable {
    unsafe { (*(&raw const MOUSE_TABLE)).as_ref().unwrap() }
}

fn mouse_cap() -> capability::CapId {
    MOUSE_CAP.load(core::sync::atomic::Ordering::SeqCst)
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

// Phase 9 deliverable 5's real, live-crash-found fix (a real double
// fault under `-smp`, root-caused via disassembly + addr2line, not
// guessed): these used to be TWO PLAIN GLOBAL `static mut`s, shared by
// every core and every in-flight syscall. That was already a latent
// bug even before SMP existed -- `syscall_entry`'s own comment below
// explains dispatch is deliberately preemptible (`sti` mid-entry, so a
// blocking syscall like `ipc::send`/`receive` doesn't deadlock the
// timer) -- meaning a SECOND thread's OWN syscall could enter and
// OVERWRITE `USER_RSP_SCRATCH` while a FIRST thread's syscall was still
// suspended mid-dispatch; when the first thread resumed and read
// `USER_RSP_SCRATCH` back to return to user space, it got the SECOND
// thread's value instead of its own. Real cross-core concurrency
// (deliverable 3) made this dramatically easier to hit (now genuinely
// simultaneous, not just interleaved by one core's own preemption), but
// the bug shape is the same one `gdt.rs`'s `TSS.RSP0`/`IOPB` history
// already found and fixed twice — a single shared slot standing in for
// what must be per-execution-context state.
//
// Fixed properly this time, with the ARCHITECTURALLY INTENDED mechanism
// for exactly this problem: `SWAPGS` + `IA32_KERNEL_GS_BASE`, one real
// per-CPU two-`u64` slot per core (`PER_CPU_SYSCALL_SCRATCH`), indexed
// by `smp::current_cpu_index()` ONLY at MSR-setup time (once per core,
// in `init()` — see its own doc comment), never inside the hot
// `syscall_entry` path itself, which instead reaches its own slot via
// `swapgs` + GS-relative addressing — real hardware support for "find
// my own per-core data with no general-purpose register free to use as
// an index yet", which is exactly the situation at the very first
// instruction of a SYSCALL entry.
#[repr(C)]
struct PerCpuSyscallScratch {
    kernel_rsp: u64,
    user_rsp_scratch: u64,
}
const PER_CPU_SYSCALL_SCRATCH_ZERO: PerCpuSyscallScratch = PerCpuSyscallScratch { kernel_rsp: 0, user_rsp_scratch: 0 };
static mut PER_CPU_SYSCALL_SCRATCH: [PerCpuSyscallScratch; crate::smp::MAX_CPUS] =
    [PER_CPU_SYSCALL_SCRATCH_ZERO; crate::smp::MAX_CPUS];

const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

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
    unsafe {
        (&mut *&raw mut PER_CPU_SYSCALL_SCRATCH)[crate::smp::current_cpu_index()].kernel_rsp = rsp;
    }
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
        10 => {
            // Phase 10: the real ring-3-reachable counterpart to
            // `socket_demo.rs`'s kernel-thread proof -- a0 = the
            // CALLER's own `CapId` for a `Socket` capability it was
            // granted at spawn. Resolves it against the calling
            // thread's OWN cap_table (same `resolve_current_capability`
            // every other per-process check here uses), requiring
            // `Rights::SEND` — the same bit `driver.rs::
            // create_socket_capability`'s doc comment explains a
            // Socket is deliberately gated on. A revoked or
            // never-granted capability refuses here exactly as it
            // does for `socket_demo.rs`'s kernel-thread holder,
            // because it is the identical check.
            match thread::resolve_current_capability(a0 as capability::CapId, capability::Rights::SEND) {
                Ok(cap) => match capability::object_kind(cap.object_id) {
                    Some(capability::KernelObjectKind::Socket { .. }) => {
                        klog_info!("SYSCALL_SOCKET_USE_OK cap={}", a0);
                        0
                    }
                    _ => {
                        klog_info!("SYSCALL_SOCKET_USE_WRONG_KIND cap={}", a0);
                        u64::MAX
                    }
                },
                Err(_) => {
                    klog_info!("SYSCALL_SOCKET_USE_DENIED cap={}", a0);
                    u64::MAX
                }
            }
        }
        11 => {
            // Phase 12 (docs/ROADMAP.md Sec5, deliverable 1, exit
            // criterion 1): SYS_SURFACE_FILL -- a0 = the CALLER's own
            // CapId for a Surface capability, a1 = the real color to
            // fill it with. Real cross-process isolation mechanism:
            // `compositor::syscall_fill_surface` resolves `a0`
            // against the CALLING thread's OWN cap_table only (same
            // `resolve_current_capability` every other per-process
            // check here uses) -- a process can never name, guess, or
            // otherwise reach another process's Surface through this
            // call, because CapIds are table-local indices, not
            // global handles. The actual pixel write happens here, in
            // the kernel, bounded to exactly the resolved Surface's
            // own real x/y/width/height -- the calling process never
            // receives a framebuffer pointer at all.
            crate::compositor::syscall_fill_surface(a0 as capability::CapId, a1 as u32)
        }
        12 => {
            // Phase 12 exit criterion 4: SYS_IPC_TRY_RECEIVE -- a0 = the
            // CALLER's own CapId for an IpcEndpoint capability (e.g. the
            // one `input_routing::register_window_input` granted it).
            // Same per-process resolve discipline as syscall 11: this
            // can only ever check the CALLING thread's own cap_table,
            // never another process's. Non-blocking by design (`ipc::
            // try_receive`'s own doc) -- "nothing has arrived yet" is a
            // real, expected outcome for a window that doesn't
            // currently hold input focus, not an error. Returns the
            // real message payload's low 64 bits on success, u64::MAX
            // for both "no message pending" and "capability denied" --
            // this syscall never carries data that could collide with
            // u64::MAX (a scancode is a single real byte).
            match thread::resolve_current_capability(a0 as capability::CapId, capability::Rights::RECEIVE) {
                Ok(cap) => match ipc::try_receive_on_object(cap.object_id) {
                    Some(msg) => msg.data[0],
                    None => u64::MAX,
                },
                Err(_) => {
                    klog_info!("SYSCALL_IPC_TRY_RECEIVE_DENIED cap={}", a0);
                    u64::MAX
                }
            }
        }
        13 => {
            // Syscall 13: SYS_ROUTE_KEY_EVENT -- caller must hold PortIoRange for port 0x60
            if !thread::current_has_port_access(0x60) {
                klog_info!("SYS_ROUTE_KEY_EVENT_DENIED");
                crate::audit::record(crate::audit::AuditEvent::Denied { cap_id: 0 });
                return u64::MAX;
            }
            crate::input_routing::deliver_key_event(a0 as u8);
            0
        }
        14 => {
            // Phase 12 deliverable 4: SYS_SURFACE_DRAW_TEXT -- a0 = the
            // CALLER's own CapId for a Surface, a1 = the vaddr (in the
            // CALLER's own address space) of a SurfaceTextRequest.
            // Real per-process isolation identical to syscall 11: the
            // Surface capability and the request/text pointers are all
            // resolved/validated against the CALLING thread's own
            // context only. See compositor::syscall_draw_text's own
            // doc for the real bounds enforcement.
            crate::compositor::syscall_draw_text(a0 as capability::CapId, a1)
        }
        15 => {
            // Real window objects: SYS_WINDOW_MOVE -- a0 = the CALLER's
            // own CapId for its Surface, a1 = the new real screen
            // position packed as (x << 32) | (y & 0xFFFFFFFF), each
            // half a real i32 (window positions are small and can go
            // slightly negative during clamping math, hence signed --
            // never negative by the time it reaches the framebuffer,
            // `window_manager::move_window` clamps on-screen). Same
            // per-process isolation as every other Surface syscall
            // here: resolved only against the CALLING thread's own
            // cap_table, so a process can only ever move its OWN
            // window.
            let new_x = ((a1 >> 32) as u32) as i32;
            let new_y = (a1 as u32) as i32;
            crate::compositor::syscall_move_window(a0 as capability::CapId, new_x, new_y)
        }
        16 => {
            // Real, disclosed latency fix: SYS_SURFACE_PRESENT -- a0 =
            // the CALLER's own CapId for its Surface. `SYS_SURFACE_FILL`
            // (11) and `SYS_SURFACE_DRAW_TEXT` (14) now only write into
            // that window's own in-memory buffer; nothing reaches the
            // real framebuffer until this syscall recomposites it. A
            // caller does a whole batch of fill/draw_text calls, then
            // presents exactly once -- see `compositor::
            // syscall_present_window`'s own doc for the real
            // per-keystroke redraw cost this replaces.
            //
            // a1 == 0: full recomposite (all windows, title bar
            // included) -- the ONLY safe choice the first time a
            // window appears, or after anything that could affect more
            // than one row of ITS OWN content.
            // a1 != 0: a real, disclosed partial-redraw fast path --
            // packed as `(local_y << 32) | local_height`, both real
            // u32s (a text row is always small and non-negative, no
            // sign-extension concern here unlike SYS_WINDOW_MOVE's
            // packed i32 halves) -- see `window_manager::
            // present_partial`'s own doc for why this exists and its
            // real, disclosed "no overlapping windows yet" scope limit.
            let dirty = if a1 == 0 {
                None
            } else {
                Some(((a1 >> 32) as u32, a1 as u32))
            };
            crate::compositor::syscall_present_window(a0 as capability::CapId, dirty)
        }
        17 => {
            // Real block/file-I/O-for-apps path: SYS_FILE_SERVICE_REQUEST
            // -- a0 = the real ext2 inode number to request (see
            // `file_service.rs`'s own module doc). Returns a real
            // request id (poll for it via syscall 19), or 0 if no
            // serving process has ever registered.
            crate::file_service::request_file(a0 as u32)
        }
        18 => {
            // SYS_FILE_SERVICE_REPLY -- called by the SERVING process
            // (e.g. `virtio_blk_driver`) only. a1 = vaddr of a real
            // `FileReplyRequest` in the CALLER's own mapped memory.
            crate::file_service::syscall_reply(vmm::current_cr3(), a1)
        }
        19 => {
            // SYS_FILE_SERVICE_POLL -- called by the REQUESTING process
            // (e.g. `file_manager`). a0 = the request id `SYS_FILE_
            // SERVICE_REQUEST` returned; a1 = vaddr of a real
            // `FilePollRequest` in the CALLER's own mapped memory.
            crate::file_service::syscall_poll(vmm::current_cr3(), a0, a1)
        }
        20 => {
            // Real GUI mouse support: SYS_MOUSE_WAIT_INTERRUPT -- same
            // real, capability-gated block-until-IRQ12 mechanism
            // syscall 5 already established for the keyboard, mirrored
            // for the mouse's own fixed InterruptLine capability.
            match driver::wait_interrupt(mouse_table(), mouse_cap()) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        21 => {
            // SYS_MOUSE_ACK_INTERRUPT -- mirrors syscall 6.
            match driver::ack_interrupt(mouse_table(), mouse_cap()) {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        22 => {
            // SYS_MOUSE_REPORT -- caller must hold PortIoRange for port 0x60
            if !thread::current_has_port_access(0x60) {
                klog_info!("SYS_MOUSE_REPORT_DENIED");
                crate::audit::record(crate::audit::AuditEvent::Denied { cap_id: 0 });
                return u64::MAX;
            }
            let dx = (a0 as u16) as i16;
            let dy = ((a0 >> 16) as u16) as i16;
            let buttons = ((a0 >> 32) as u8) & 0x1;
            crate::window_manager::report_mouse(dx as i32, dy as i32, buttons != 0);
            0
        }
        23 => {
            // SYS_SURFACE_DRAW_BITMAP -- a0 = Surface CapId, a1 = SurfaceBitmapRequest vaddr
            crate::compositor::syscall_draw_bitmap(a0 as capability::CapId, a1)
        }
        24 => {
            // SYS_FILE_SERVICE_WRITE -- a0 = inode, a1 = FileWriteRequest vaddr
            crate::file_service::syscall_write(vmm::current_cr3(), a0 as u32, a1)
        }
        25 => {
            // SYS_FILE_SERVICE_GET_WRITE_DATA -- a0 = request_id, a1 = out_vaddr
            crate::file_service::syscall_get_write_data(vmm::current_cr3(), a0, a1, 1024)
        }
        26 => {
            // SYS_NET_SERVICE_REQUEST -- a0 = Socket CapId
            crate::net_service::syscall_request(a0 as capability::CapId)
        }
        27 => {
            // SYS_NET_SERVICE_REPLY -- a1 = NetReplyRequest vaddr
            crate::net_service::syscall_reply(vmm::current_cr3(), a1)
        }
        28 => {
            // SYS_NET_SERVICE_POLL -- a0 = request_id, a1 = NetPollRequest vaddr
            crate::net_service::syscall_poll(vmm::current_cr3(), a0, a1)
        }
        29 => {
            // SYS_YIELD -- no arguments. A ring-3 thread that has finished
            // all available work (e.g. drained its IPC queue and has no new
            // key events) calls this to immediately relinquish the rest of
            // its scheduling quantum rather than busy-spinning until the
            // next APIC timer interrupt. Real, evidence-backed latency fix:
            // combined with the 1_000_000 timer quantum reduction (apic.rs)
            // and the 32-slot IPC queue (ipc.rs), cooperative yield ensures
            // the keyboard driver's IPC send finds the focused window thread
            // in READY state within microseconds of the next interrupt,
            // instead of waiting up to one full ~15ms quantum.
            thread::schedule();
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
        // Real fix (see PER_CPU_SYSCALL_SCRATCH's own doc comment for
        // the live crash this replaced): `swapgs` first -- this core's
        // OWN `IA32_KERNEL_GS_BASE` (set once, per-core, by `init()`)
        // becomes the active GS base, so `gs:[0]`/`gs:[8]` below reach
        // THIS core's own two-`u64` scratch slot, never another core's.
        // No general-purpose register is touched or needed to compute
        // that address -- exactly why `swapgs` is the real, intended
        // mechanism for this exact "which core am I" problem at the
        // very first instruction of a syscall.
        //
        // Real SECOND bug found via a live crash (page fault, cr2=0x8 —
        // a write through a NULL-based GS address) even after the
        // per-core MSR setup above was fully correct on every core: the
        // "active GS base" toggle `swapgs` flips is genuinely PER-CORE
        // hardware state, invisible to `switch_to`/the scheduler
        // entirely. This kernel's own syscall dispatch is deliberately
        // PREEMPTIBLE (`sti` a few lines below, so a blocking syscall
        // like `ipc::send`/`receive` doesn't stall the timer forever) —
        // if thread A gets preempted mid-dispatch while this core's
        // "active GS" is still toggled to KERNEL (never restored, since
        // A's own matching exit-side swap hasn't run yet), and thread B
        // (a DIFFERENT ring-3 thread) then takes ITS OWN `syscall` on
        // this SAME core, B's entry `swapgs` toggles the core's ALREADY
        // -kernel state back to USER instead of TO kernel — so B's
        // `gs:[8]`/`gs:[0]` accesses land on B's own (zeroed, never
        // configured) user GS base, i.e. address 8 and 0. Fixed by
        // toggling BACK to user-active immediately after this narrow,
        // interrupts-still-disabled pair of GS-relative accesses,
        // rather than leaving kernel-GS "held" active across the whole
        // (preemptible) dispatch — the exit path below does the
        // matching toggle-in/toggle-out pair again, just before it
        // needs `gs:[8]` one more time. Every GS toggle pair is now
        // fully atomic with respect to any other thread's own entry on
        // this core, regardless of what gets preempted in between.
        "swapgs",
        // Real bug found and fixed THIS session, root-caused live via
        // `text_editor`'s own crash (a real ring-3 #PF, cr2 landing near
        // a totally unrelated thread's own stack region): the OLD
        // sequence here (`mov gs:[8], rsp` then later `mov rsp, gs:[8]`
        // at exit) stashes the user RSP in ONE shared PER-CORE slot.
        // That is safe ONLY as long as no other thread's own syscall
        // entry can occur between THIS thread's entry and its own exit
        // — true for an ordinary non-blocking syscall on its own (see
        // this file's own earlier fix removing the unconditional `sti`
        // during dispatch), but NOT when a DIFFERENT thread is
        // currently sitting inside its own BLOCKING wait (`ipc::
        // spin_yield`/`interrupt_forward::wait_for_interrupt`'s own
        // narrow `sti; hlt; cli`) — interrupts ARE briefly live there,
        // so a timer tick can let a THIRD thread's syscall run,
        // overwrite this SAME shared slot, and hand the blocked
        // thread's eventual `sysretq` a completely wrong stack pointer
        // once it wakes and finishes. `terminal_emulator` apparently
        // never won that race in testing; `text_editor`'s different
        // syscall cadence (more calls per keystroke: fill/draw_text/
        // present, not just one) did, reliably. Real fix: stash the
        // user RSP on THIS THREAD'S OWN kernel stack instead of a
        // shared per-core slot -- `gs:[0]` (`kernel_rsp`) is already
        // updated to the CURRENT thread's own kernel stack top on
        // every real context switch (`thread.rs::schedule_locked`,
        // unconditionally, every switch), so a slot addressed relative
        // to it is automatically per-thread, with zero extra
        // bookkeeping: no two threads can ever collide on it, because
        // no two threads ever share a kernel stack.
        "mov r10, rsp",              // save the real user RSP in a scratch reg (r10 carries no syscall arg in this kernel's 3-arg ABI, free to clobber here)
        "mov rsp, gs:[0]",           // switch onto THIS thread's own current kernel stack
        "sub rsp, 8",                // reserve THIS thread's own one-slot stash, on ITS OWN stack -- never shared with any other thread
        "mov [rsp], r10",            // stash the real user RSP there
        "swapgs",                    // restore user-GS-active state -- see the real bug this fixes, above
        "push rcx",                  // user RIP (SYSCALL-saved) — must survive to sysretq
        "push r11",                  // user RFLAGS (SYSCALL-saved) — same
        // Real bug found and fixed THIS session, replacing an earlier
        // real fix that turned out to be incomplete: this used to `sti`
        // right here, unconditionally, so a blocking syscall (ipc::
        // send/receive's own spin-yield via hlt) wouldn't deadlock the
        // timer. That real fix (IA32_FMASK clears IF on SYSCALL entry)
        // was correct as far as it went, but it also reopened the exact
        // per-core race this module's own doc already fixed once for
        // the GS-base-active TOGGLE: with interrupts live for the
        // WHOLE dispatch, a second thread's own syscall entry on this
        // SAME core can preempt-and-interleave with this one, and
        // BOTH entries stash their real user RSP into the SAME
        // per-core `gs:[8]` slot (see entry's own comment) — whichever
        // one restores last wins, handing the OTHER thread's exit a
        // completely wrong stack pointer. Reproduced live bringing up
        // Phase 13's `terminal_emulator`: its own tight, real,
        // non-blocking `SYS_IPC_TRY_RECEIVE` poll loop (syscall 12)
        // interleaving with `keyboard_driver`'s own real routing
        // syscall (13) on the same core crashed the terminal with a
        // real ring-3 write #PF just past its own mapped stack, every
        // time. Real fix: dispatch no longer re-enables interrupts at
        // all -- every syscall this kernel defines today is either
        // genuinely non-blocking (safe, and now fully atomic with
        // respect to this core's own preemption) or blocks via a
        // `hlt`-based spin (`ipc::spin_yield`, `interrupt_forward::
        // wait_for_interrupt`) that now does its OWN narrow `sti`/`hlt`
        // pairing around just the wait itself (see those functions'
        // own updated doc) — the timer can still fire and this core
        // can still be preempted DURING a blocking wait, but no longer
        // for the full duration of an ordinary, non-blocking syscall,
        // which is what this specific race needed.
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
        //
        // Real bug found and fixed THIS session, disassembly-confirmed:
        // r14/r15/rbx/rbp were left OUT of that same fix, on the
        // assumption that `syscall_dispatch` being a normal Rust
        // `extern "C" fn` would preserve them itself per the SysV ABI
        // (true in general -- a compliant callee saves whatever
        // callee-saved registers it actually uses). In practice, one of
        // Phase 13's own `terminal_emulator` local variables
        // (`current_len`) got allocated to r15 and came back corrupted
        // after a single `SYS_IPC_TRY_RECEIVE` syscall — root-caused via
        // a real disassembly of the faulting binary (capstone), which
        // showed the actual crash instruction (`mov [rsp+r15+0x30], al`
        // — the real `current[current_len] = ch` write) using a
        // garbage r15 instead of the real value 0. Whatever the exact
        // codegen path that let r14/r15/rbx/rbp leak past `dispatch`'s
        // own preservation (this naked `call` site, crossing into a
        // deep, multi-module call chain, is real, unusual territory
        // this kernel had never exercised at this frequency before),
        // the kernel's OWN entry/exit stub explicitly guaranteeing ALL
        // SIX callee-saved registers survive a syscall — not just the
        // two this module happens to reshuffle — is the real, robust
        // fix: never again trust an implicit, transitive ABI guarantee
        // for a boundary this security- and correctness-critical.
        "push r14",
        "push r15",
        "push rbx",
        "push rbp",
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
        "pop rbp",
        "pop rbx",
        "pop r15",
        "pop r14",
        "pop r11",
        "pop rcx",
        // `cli`, unconditionally, for the rest of this exit sequence --
        // interrupts are already off by this point (entry's own `sti`
        // was removed -- see that comment) so this is a no-op in the
        // common case; kept as an explicit statement of the invariant
        // the frame-building sequence below relies on.
        "cli",
        // Real fix, matching entry's own toggle-in/toggle-out pair
        // (see entry's own doc comment): swap to kernel-GS just long
        // enough to read this core's own saved user RSP back out, then
        // immediately swap back to user-GS before returning to ring 3 --
        // never leave kernel-GS "held" active across anything
        // preemptible (this whole exit sequence runs `cli`'d, so it's
        // already atomic with respect to this core's own interrupts;
        // the narrow toggle-in/toggle-out here keeps it correct with
        // respect to any OTHER thread's entry on this same core too).
        "swapgs",
        // Real fix, matching entry's own new per-thread-stack stash
        // (see entry's own doc comment above for the real cross-thread
        // race this replaces): the stash lives at [rsp] right here --
        // `call {dispatch}`'s own internal push/pop discipline is
        // self-balanced, so rsp is back to EXACTLY where entry's `sub
        // rsp, 8` left it, on THIS SAME THREAD's own kernel stack (never
        // another thread's, unlike the old shared per-core `gs:[8]`).
        "mov r10, [rsp]",            // recover the real user RSP from THIS thread's own stash slot
        "add rsp, 8",                // give back the stash slot -- rsp is now back on this thread's own kernel stack
        // Real bug found and fixed (WHPX-only, never reproducible under
        // TCG): this used to `mov rsp, r10` (switch straight onto the
        // user's own stack) then `sysretq`. SYSRET's CS/SS/RIP/RFLAGS
        // reload is NOT a single atomic stack-frame load the way IRETQ's
        // is -- it's a sequence of discrete MSR-derived register loads,
        // and on real silicon (exposed here by WHPX's real interrupt
        // timing, invisible under TCG's atomic instruction-at-a-time
        // interpreter) a hardware interrupt landing during that sequence
        // can be delivered against a torn combination of already-updated
        // and not-yet-updated segment state -- observed live as a ring-0
        // #GP (error code referencing the user data selector, GDT index
        // 5) when AHCI's own driver thread got timer-interrupted right
        // as its own SYSCALL round-trip was returning: the CPU's live SS
        // held 0x28 (the raw GDT index, RPL bits not yet applied) while
        // CS had already updated to the real 0x33 (RPL 3) user code
        // selector -- an architecturally inconsistent pair no ordinary
        // ring-3 execution could otherwise produce. IRETQ has no
        // equivalent hazard: it pops a single, fully-formed 5-word frame
        // (RIP, CS, RFLAGS, RSP, SS) already sitting in memory, with no
        // partial-update window an interrupt could land inside. Fixed by
        // building that exact frame on THIS thread's own kernel stack
        // (still fully valid at this point) instead of switching RSP
        // early, and using `iretq` instead of `sysretq` for the return
        // to ring 3 -- a small, deliberate cost (iretq's own segment/
        // privilege checks are pricier than sysret's) paid only on the
        // exit side, entry still uses the fast `syscall` path.
        "push {user_ss}",            // SS (highest address of the frame -- iretq pops SS last)
        "push r10",                  // RSP -- the user's real stack pointer
        "push r11",                  // RFLAGS (SYSCALL-saved)
        "push {user_cs}",            // CS
        "push rcx",                  // RIP (SYSCALL-saved) -- lowest address, iretq pops this first
        "swapgs",                    // restore the user's own GS base before returning to ring 3
        "iretq",
        dispatch = sym syscall_dispatch,
        user_cs = const crate::gdt::USER_CODE_SELECTOR as u64,
        user_ss = const crate::gdt::USER_DATA_SELECTOR as u64,
    );
}

// Phase 9: EFER/STAR/LSTAR/FMASK/KERNEL_GS_BASE are all genuinely
// PER-CORE hardware MSRs, not shared — a single global "already
// initialized" flag (the pre-SMP version of this) meant only whichever
// core happened to call `init()` FIRST ever actually got SYSCALL/SYSRET
// configured; every OTHER core would `#UD`-fault the first time a
// ring-3 thread scheduled onto it tried to execute `syscall`. Real,
// per-CPU idempotency instead: this is still called from the same
// driver-setup call sites as before (agent.rs, ahci.rs, init.rs, etc.),
// which already run on whatever core the scheduler happens to place
// them on — making `init()` itself per-core-aware means every core that
// ever hosts one of those threads gets correctly configured, with no
// new call sites needed anywhere.
const SYSCALL_INITIALIZED_ZERO: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SYSCALL_INITIALIZED: [core::sync::atomic::AtomicBool; crate::smp::MAX_CPUS] =
    [SYSCALL_INITIALIZED_ZERO; crate::smp::MAX_CPUS];

/// Idempotent PER CORE: init.rs's real init process and the
/// feature-gated demo_ring3 proof can both run in the same build and
/// each call this once before entering ring 3 for the first time.
/// Re-running the MSR writes with the same values on the SAME core
/// would be harmless anyway, but this avoids a confusing duplicate
/// "initialized" log line — and, correctly, does NOT skip a core just
/// because some OTHER core already ran this once.
pub fn init() {
    let cpu = crate::smp::current_cpu_index();
    if SYSCALL_INITIALIZED[cpu].swap(true, core::sync::atomic::Ordering::SeqCst) {
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

        // Real fix (see PER_CPU_SYSCALL_SCRATCH's own doc comment):
        // point THIS core's own IA32_KERNEL_GS_BASE at THIS core's own
        // scratch slot -- `syscall_entry`'s `swapgs` + `gs:[0]`/`gs:[8]`
        // then reach exactly this address, on every core, without ever
        // needing a runtime index lookup inside the hot entry path.
        let scratch_addr = (&raw const (&*&raw const PER_CPU_SYSCALL_SCRATCH)[cpu]) as u64;
        wrmsr(IA32_KERNEL_GS_BASE, scratch_addr);
    }
    klog_info!("SYSCALL/SYSRET initialized cpu_index={} (real per-core GS-based kernel-stack model)", cpu);
}
