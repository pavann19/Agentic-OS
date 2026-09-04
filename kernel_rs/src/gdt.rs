//! Minimal GDT + TSS. Port of `kernel/gdt.c`'s null/code/data descriptors,
//! extended with a TSS descriptor and one IST (Interrupt Stack Table) entry
//! — the C version had neither, and `docs/ROADMAP.md` Phase 0 explicitly
//! calls out "TSS/IST protects double fault path" as a requirement the C
//! kernel never met. A double fault on a corrupted or exhausted stack needs
//! its handler to run on a KNOWN-GOOD separate stack (the IST mechanism)
//! rather than trying to push a fault frame onto the same stack that may be
//! the reason it faulted.

#![allow(dead_code)]

use crate::klog_info;

const GDT_ENTRIES: usize = 7; // null, code, data, TSS (2 entries: TSS is 16 bytes = 2 slots)

/// Phase 9 deliverable 2: GDT/TSS/double-fault-stack are now real,
/// PER-CPU state, indexed by `smp::current_cpu_index()` /
/// `smp::MAX_CPUS` -- `smp.rs`'s own module doc already named this
/// gap explicitly ("gdt.rs's GDT/TSS ... single, unsynchronized shared
/// kernel state today"). One core's TSS.RSP0/IOPB/IST must never be
/// visible to another core the way it already had to stop being
/// visible across THREADS on one core (see this file's own
/// `set_iopb`/`set_kernel_stack` doc comments for that earlier,
/// single-core version of the same bug class) -- a shared TSS across
/// real concurrent cores would be that bug again, just races-instead-
/// of-single-threaded-races.
use crate::smp::MAX_CPUS;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct GdtEntry {
    limit_low: u16,
    base_low: u16,
    base_mid: u8,
    access: u8,
    flags_limit_high: u8,
    base_high: u8,
}

#[repr(C, packed)]
struct GdtDescriptor {
    limit: u16,
    base: u64,
}

// IOPB_PORTS: how many ports this TSS's I/O permission bitmap covers.
// 1024 (0x000-0x3FF) is enough to include COM1 (0x3F8-0x3FF), the first
// user-space driver Phase 3 targets — not all 65536 ports, since a
// bitmap only needs to extend as far as the highest port any capability
// will ever grant; the CPU treats any port past the TSS limit as
// permanently denied, which is the correct default-deny posture anyway.
pub const IOPB_PORTS: usize = 1024;
pub const IOPB_BYTES: usize = IOPB_PORTS / 8 + 1; // +1 for the mandatory trailing all-1s byte (Intel SDM)

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Tss {
    reserved0: u32,
    pub rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    pub ist: [u64; 7], // array index 0 IS the IST1 slot -- see the real bug this caused, below
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
    /// Phase 3: real per-port grants, not full IOPL=3 (which would open
    /// EVERY port to any ring-3 code, defeating the whole point of a
    /// capability-gated grant). Bit N clear (0) = port N allowed at CPL3;
    /// set (1, the default) = denied, same #GP a random port access
    /// already gets. `driver.rs::grant_port_access` clears specific bits;
    /// nothing else ever should.
    pub iopb: [u8; IOPB_BYTES],
}

// Real bug this session found via `qemu -d int`: the IDT gate's IST field
// value (1..7, what the CPU is told — "use IST1") and the array index into
// TSS.ist (0..6, since that array has no unused slot 0 — index 0 IS IST1's
// slot) are NOT the same number, despite looking like they should be.
// DOUBLE_FAULT_IST_VALUE (1) goes into the IDT gate; DOUBLE_FAULT_IST_ARRAY_INDEX
// (0) is where the stack address is actually stored. Writing the address to
// `ist[1]` (IST2's slot) while telling the CPU "use IST1" left IST1's real
// slot (index 0) zeroed — confirmed via `-d int`: SP after entering the
// double-fault vector was the SAME (already-exhausted) stack, not a switch
// to anything, exactly what "the CPU read a zeroed IST1 slot" would look like.
pub const DOUBLE_FAULT_IST_VALUE: u8 = 1;
const DOUBLE_FAULT_IST_ARRAY_INDEX: usize = 0;
const DOUBLE_FAULT_STACK_SIZE: usize = 16 * 1024;

const GDT_ENTRY_ZERO: GdtEntry = GdtEntry {
    limit_low: 0,
    base_low: 0,
    base_mid: 0,
    access: 0,
    flags_limit_high: 0,
    base_high: 0,
};
const GDT_ZERO: [GdtEntry; GDT_ENTRIES] = [GDT_ENTRY_ZERO; GDT_ENTRIES];

const TSS_ZERO: Tss = Tss {
    reserved0: 0,
    rsp0: 0,
    rsp1: 0,
    rsp2: 0,
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: 0,
    iopb: [0xFF; IOPB_BYTES], // default-deny every port
};

/// One GDT and one TSS PER real CPU (see this file's module-level doc
/// comment above `GDT_ENTRIES` for why). `init_for_cpu(index)` builds
/// and loads exactly one slot; every other function here that used to
/// touch the single global `TSS` now resolves `smp::current_cpu_index()`
/// first and touches only that CPU's own slot.
static mut GDTS: [[GdtEntry; GDT_ENTRIES]; MAX_CPUS] = [GDT_ZERO; MAX_CPUS];
static mut TSSES: [Tss; MAX_CPUS] = [TSS_ZERO; MAX_CPUS];

// A double-fault-dedicated stack, statically allocated (not via the PMM —
// this must exist and be mapped BEFORE the VMM/PMM's own correctness can
// be trusted, since a double fault is exactly the kind of thing that can
// happen while THOSE are still being debugged; a .bss static is mapped by
// the same kernel-segment mapping every other kernel .bss page gets).
#[repr(align(16))]
struct DoubleFaultStack([u8; DOUBLE_FAULT_STACK_SIZE]);
const DF_STACK_ZERO: DoubleFaultStack = DoubleFaultStack([0; DOUBLE_FAULT_STACK_SIZE]);
/// One double-fault stack PER CPU -- a shared one would defeat its own
/// purpose the instant two cores double-fault concurrently (each core
/// pushing its own exception frame onto the SAME "known-good" stack is
/// exactly the corruption this mechanism exists to prevent in the
/// first place).
static mut DOUBLE_FAULT_STACKS: [DoubleFaultStack; MAX_CPUS] = [DF_STACK_ZERO; MAX_CPUS];

fn set_entry(cpu: usize, index: usize, base: u32, limit: u32, access: u8, flags: u8) {
    unsafe {
        let gdt = &mut (&mut *&raw mut GDTS)[cpu];
        gdt[index].base_low = (base & 0xFFFF) as u16;
        gdt[index].base_mid = ((base >> 16) & 0xFF) as u8;
        gdt[index].base_high = ((base >> 24) & 0xFF) as u8;
        gdt[index].limit_low = (limit & 0xFFFF) as u16;
        gdt[index].flags_limit_high = (((limit >> 16) & 0x0F) as u8) | (flags & 0xF0);
        gdt[index].access = access;
    }
}

/// Sets a 16-byte (two-slot) TSS descriptor at `index`/`index+1` in CPU
/// `cpu`'s own GDT. Unlike a normal code/data descriptor, a TSS
/// descriptor's base is a full 64-bit address (there is no 64-bit-mode
/// long-mode segmentation to abbreviate it), so it needs the extra
/// slot for the high 32 bits.
fn set_tss_entry(cpu: usize, index: usize, base: u64, limit: u32) {
    unsafe {
        let gdt = &mut (&mut *&raw mut GDTS)[cpu];
        let low = (&raw mut gdt[index]) as *mut u8;
        let base32 = base as u32;
        let base_high32 = (base >> 32) as u32;

        core::ptr::write_unaligned(low.add(0) as *mut u16, (limit & 0xFFFF) as u16);
        core::ptr::write_unaligned(low.add(2) as *mut u16, (base32 & 0xFFFF) as u16);
        *low.add(4) = ((base32 >> 16) & 0xFF) as u8;
        *low.add(5) = 0x89; // present, DPL0, type=0x9 (64-bit TSS, available)
        *low.add(6) = (((limit >> 16) & 0x0F) as u8) | 0x00;
        *low.add(7) = ((base32 >> 24) & 0xFF) as u8;
        // Second slot: high 32 bits of base, then reserved.
        core::ptr::write_unaligned(low.add(8) as *mut u32, base_high32);
        core::ptr::write_unaligned(low.add(12) as *mut u32, 0);
    }
}

// Ring 3 (Phase 1 item). Selectors include the RPL=3 bits callers need
// when loading CS/SS — `USER_CODE_SELECTOR`/`USER_DATA_SELECTOR` are
// ready to load directly, not raw GDT indices.
pub const USER_DATA_SELECTOR: u16 = (5 << 3) | 3; // 0x2B
pub const USER_CODE_SELECTOR: u16 = (6 << 3) | 3; // 0x33

/// Phase 9 deliverable 2: builds and loads CPU `cpu`'s OWN GDT+TSS+
/// double-fault stack — real, per-core state, not a shared global
/// anymore (see this file's module-level doc comment on why a shared
/// TSS across real concurrent cores is exactly the RSP0/IOPB bug class
/// already found and fixed once for single-core-multi-THREAD, just one
/// level up). The BSP calls this with `cpu = 0` at boot, before
/// `smp::current_cpu_index()` can even resolve (no APs exist yet, so 0
/// is trivially correct); every AP calls it for its OWN assigned index
/// from `smp.rs::ap_entry`, before anything else runs on that core.
pub fn init_for_cpu(cpu: usize) {
    unsafe {
        set_entry(cpu, 0, 0, 0, 0, 0); // null
        set_entry(cpu, 1, 0, 0xFFFFF, 0x9A, 0xA0); // kernel code, selector 0x08
        set_entry(cpu, 2, 0, 0xFFFFF, 0x92, 0x80); // kernel data, selector 0x10
        // DPL=3 versions of the same access-byte pattern as the kernel
        // descriptors above (bits 5-6 = DPL, set to 11 instead of 00):
        // kernel code 0x9A -> user code 0xFA; kernel data 0x92 -> user
        // data 0xF2. User data MUST come before user code at consecutive
        // indices (5, 6) here — that ordering is what SYSCALL/SYSRET's
        // STAR MSR will rely on later; getting it right now avoids
        // reshuffling GDT indices when that item lands.
        set_entry(cpu, 5, 0, 0xFFFFF, 0xF2, 0x80); // user data, selector 0x28
        set_entry(cpu, 6, 0, 0xFFFFF, 0xFA, 0xA0); // user code, selector 0x30

        let tss = &mut (&mut *&raw mut TSSES)[cpu];
        let df_stack = &mut (&mut *&raw mut DOUBLE_FAULT_STACKS)[cpu];
        let df_stack_top = (&raw const df_stack.0) as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
        tss.ist[DOUBLE_FAULT_IST_ARRAY_INDEX] = df_stack_top;
        // iomap_base points AT the real iopb field now (Phase 3), not past
        // the end of the struct — computed via pointer arithmetic so it
        // stays correct if Tss's layout ever changes, rather than a
        // hand-counted offset that could silently drift out of sync.
        let tss_base_addr = (&raw const *tss) as u64;
        let iopb_addr = (&raw const tss.iopb) as u64;
        tss.iomap_base = (iopb_addr - tss_base_addr) as u16;

        let tss_base = (&raw const *tss) as u64;
        let tss_limit = (core::mem::size_of::<Tss>() - 1) as u32;
        set_tss_entry(cpu, 3, tss_base, tss_limit); // selector 0x18-0x20, TSS uses slots 3+4

        let gdtr = GdtDescriptor {
            limit: (core::mem::size_of::<[GdtEntry; GDT_ENTRIES]>() - 1) as u16,
            base: (&raw const (&*&raw const GDTS)[cpu]) as u64,
        };
        core::arch::asm!("lgdt [{}]", in(reg) &gdtr, options(readonly, nostack, preserves_flags));

        // Reload CS via a far return (long-mode has no far jmp immediate
        // form in 64-bit code without relying on retf), then reload data
        // segments. Matches kernel/gdt.c's approach.
        core::arch::asm!(
            "push 0x08",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ss, ax",
            out("rax") _,
            options(preserves_flags)
        );

        core::arch::asm!("ltr ax", in("ax") 0x18u16, options(nostack, preserves_flags));
    }
    klog_info!("GDT+TSS initialized for cpu_index={} (double-fault IST stack ready)", cpu);
}

/// Real bug found and fixed (Phase 7's shell -- its `rawin` command,
/// meant to demonstrate a DENIED out-of-grant port read, instead
/// SUCCEEDED reading port 0x64, a port only `keyboard_driver` had ever
/// been granted): `allow_port`/`deny_port` used to mutate the single,
/// GLOBAL, CPU-visible `TSS.iopb` directly. Since there is only ONE live
/// TSS on this single-core kernel, ANY port ever granted to ANY driver
/// stayed permanently open to EVERY OTHER ring-3 process from then on --
/// the exact same class of bug already found and fixed once for
/// TSS.RSP0 (see thread.rs's own `schedule_locked` doc comment on that
/// investigation), just never re-checked for the IOPB. Fixed the same
/// way: each `Thread` now owns its OWN `iopb` bitmap
/// (`thread::Thread::iopb`), `set_iopb` below copies the INCOMING
/// thread's own bitmap into the one live TSS on every scheduler switch
/// (mirroring `set_kernel_stack`'s existing per-switch reload), and
/// `allow_port_bits`/`deny_port_bits` below operate on a caller-owned
/// bitmap array, not `TSS.iopb` directly -- `driver.rs::grant_port_access`
/// now mutates the CALLING thread's own copy (via
/// `thread::allow_port_for_current`) and pushes it live immediately, not
/// the shared global array every other thread would also see.

/// Copies `bitmap` into the CURRENTLY RUNNING CORE's OWN live TSS's
/// IOPB -- called on every scheduler switch (thread.rs, which always
/// runs on the core it's switching) and once by
/// `driver.rs::grant_port_access` for an immediate live update on the
/// granting thread's own first entry into ring 3. Phase 9 deliverable
/// 2: resolves `smp::current_cpu_index()` first -- a real, necessary
/// change from the single-core version, since a thread being switched
/// in on core 2 must never touch core 0's TSS.
pub fn set_iopb(bitmap: &[u8; IOPB_BYTES]) {
    unsafe {
        (&mut *&raw mut TSSES)[crate::smp::current_cpu_index()].iopb = *bitmap;
    }
}

/// Clears port `port`'s bit in `bitmap` (a caller-owned array — a
/// `Thread`'s own IOPB copy, never `TSS.iopb` directly), allowing ring-3
/// `in`/`out` on it once that bitmap is actually loaded via `set_iopb`.
/// Callers MUST have already checked a capability grants this — this
/// function itself has no notion of capabilities, it's the mechanism
/// `driver.rs::grant_port_access` wraps with the actual permission check.
pub fn allow_port_bits(bitmap: &mut [u8; IOPB_BYTES], port: u16) {
    let p = port as usize;
    if p >= IOPB_PORTS {
        return; // outside the bitmap's range -- permanently denied regardless
    }
    bitmap[p / 8] &= !(1 << (p % 8));
}

/// Sets port `port`'s bit back to denied in `bitmap`. Used by
/// revocation — a driver whose port-I/O capability is revoked loses
/// ACTUAL hardware access the next time its own bitmap is loaded, not
/// just the capability bookkeeping.
pub fn deny_port_bits(bitmap: &mut [u8; IOPB_BYTES], port: u16) {
    let p = port as usize;
    if p >= IOPB_PORTS {
        return;
    }
    bitmap[p / 8] |= 1 << (p % 8);
}

/// Sets TSS.RSP0 — the kernel stack the CPU switches to automatically on
/// any ring3->ring0 transition (interrupt, exception, or a future syscall
/// entry that relies on it). NOW wired into the scheduler per-thread
/// (`thread.rs::schedule_locked` calls this on every switch, to the
/// INCOMING thread's own kernel stack) — closing a real bug the previous
/// version of this comment predicted: with multiple ring-3 threads alive
/// concurrently, a single global RSP0 set once by each thread's own setup
/// code meant whichever set it last silently corrupted every OTHER
/// ring-3 thread's exception handling. Driver-setup code (user_driver.rs,
/// init.rs) still calls this once before its OWN first ring-3 entry too —
/// harmless now (the very next schedule() tick re-asserts the correct
/// per-thread value regardless), kept mainly so a thread interrupted
/// before ever being scheduled out even once still has a correct value.
/// Phase 9 deliverable 2: same core-resolution fix as `set_iopb` above
/// — writes the CURRENTLY RUNNING CORE's OWN TSS.RSP0, never a shared
/// global one.
pub fn set_kernel_stack(rsp0: u64) {
    unsafe {
        (&mut *&raw mut TSSES)[crate::smp::current_cpu_index()].rsp0 = rsp0;
    }
}
