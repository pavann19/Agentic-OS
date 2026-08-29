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

#[repr(C, packed)]
pub struct Tss {
    reserved0: u32,
    pub rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    pub ist: [u64; 7], // IST[0] unused (per spec, index 1-7 map to IST fields 1-7)
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
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

static mut GDT: [GdtEntry; GDT_ENTRIES] = [GdtEntry {
    limit_low: 0,
    base_low: 0,
    base_mid: 0,
    access: 0,
    flags_limit_high: 0,
    base_high: 0,
}; GDT_ENTRIES];

static mut TSS: Tss = Tss {
    reserved0: 0,
    rsp0: 0,
    rsp1: 0,
    rsp2: 0,
    reserved1: 0,
    ist: [0; 7],
    reserved2: 0,
    reserved3: 0,
    iomap_base: 0,
};

// A double-fault-dedicated stack, statically allocated (not via the PMM —
// this must exist and be mapped BEFORE the VMM/PMM's own correctness can
// be trusted, since a double fault is exactly the kind of thing that can
// happen while THOSE are still being debugged; a .bss static is mapped by
// the same kernel-segment mapping every other kernel .bss page gets).
#[repr(align(16))]
struct DoubleFaultStack([u8; DOUBLE_FAULT_STACK_SIZE]);
static mut DOUBLE_FAULT_STACK: DoubleFaultStack = DoubleFaultStack([0; DOUBLE_FAULT_STACK_SIZE]);

fn set_entry(index: usize, base: u32, limit: u32, access: u8, flags: u8) {
    unsafe {
        GDT[index].base_low = (base & 0xFFFF) as u16;
        GDT[index].base_mid = ((base >> 16) & 0xFF) as u8;
        GDT[index].base_high = ((base >> 24) & 0xFF) as u8;
        GDT[index].limit_low = (limit & 0xFFFF) as u16;
        GDT[index].flags_limit_high = (((limit >> 16) & 0x0F) as u8) | (flags & 0xF0);
        GDT[index].access = access;
    }
}

/// Sets a 16-byte (two-slot) TSS descriptor at `index`/`index+1`. Unlike a
/// normal code/data descriptor, a TSS descriptor's base is a full 64-bit
/// address (there is no 64-bit-mode long-mode segmentation to abbreviate
/// it), so it needs the extra slot for the high 32 bits.
fn set_tss_entry(index: usize, base: u64, limit: u32) {
    unsafe {
        let low = &mut GDT[index] as *mut GdtEntry as *mut u8;
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

pub fn init() {
    unsafe {
        set_entry(0, 0, 0, 0, 0); // null
        set_entry(1, 0, 0xFFFFF, 0x9A, 0xA0); // kernel code, selector 0x08
        set_entry(2, 0, 0xFFFFF, 0x92, 0x80); // kernel data, selector 0x10

        let df_stack_top =
            DOUBLE_FAULT_STACK.0.as_ptr() as u64 + DOUBLE_FAULT_STACK_SIZE as u64;
        TSS.ist[DOUBLE_FAULT_IST_ARRAY_INDEX] = df_stack_top;
        TSS.iomap_base = core::mem::size_of::<Tss>() as u16; // no I/O bitmap

        let tss_base = &TSS as *const Tss as u64;
        let tss_limit = (core::mem::size_of::<Tss>() - 1) as u32;
        set_tss_entry(3, tss_base, tss_limit); // selector 0x18-0x20, TSS uses slots 3+4

        let gdtr = GdtDescriptor {
            limit: (core::mem::size_of::<[GdtEntry; GDT_ENTRIES]>() - 1) as u16,
            base: &GDT as *const _ as u64,
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
    klog_info!("GDT+TSS initialized (double-fault IST stack ready)");
}
