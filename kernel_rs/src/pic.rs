//! Legacy 8259 PIC — remapped (so any spurious PIC IRQ that slips through
//! lands on a vector that isn't a CPU exception, matching `kernel/pic.c`'s
//! own reasoning) and then fully masked, since Phase 0 uses the APIC timer
//! (`apic.rs`) instead. The PIC and APIC being simultaneously active is a
//! classic source of "why did I get two interrupts for one timer tick"
//! bugs — masking it out here avoids that class of problem entirely rather
//! than trying to coordinate both.

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// IRQ1 (PS/2 keyboard) after the 0x20/0x28 remap main.rs uses --
/// offset1(0x20) + IRQ1 = 0x21. Distinct from apic::TIMER_VECTOR (0x20),
/// no collision.
pub const KEYBOARD_VECTOR: u8 = 0x21;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

unsafe fn io_wait() {
    outb(0x80, 0);
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}

pub fn remap_and_mask_all(offset1: u8, offset2: u8) {
    unsafe {
        outb(PIC1_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(PIC2_COMMAND, ICW1_INIT | ICW1_ICW4);
        io_wait();
        outb(PIC1_DATA, offset1);
        io_wait();
        outb(PIC2_DATA, offset2);
        io_wait();
        outb(PIC1_DATA, 4);
        io_wait();
        outb(PIC2_DATA, 2);
        io_wait();
        outb(PIC1_DATA, ICW4_8086);
        io_wait();
        outb(PIC2_DATA, ICW4_8086);
        io_wait();
        // Mask every line — nothing on this PIC is handled yet, and the
        // APIC timer is what Phase 0 actually uses.
        outb(PIC1_DATA, 0xFF);
        outb(PIC2_DATA, 0xFF);
    }
    crate::klog_info!("Legacy PIC remapped to 0x{:x}/0x{:x} and fully masked", offset1, offset2);
}

/// Unmasks one specific IRQ line (0-15) — Phase 3's PS/2 keyboard driver
/// needs IRQ1 real, while every OTHER legacy PIC line stays masked
/// (still fully correct for the APIC-timer-only reasoning above: this
/// clears exactly one bit, not the blanket mask). IRQ 0-7 live on PIC1,
/// 8-15 on PIC2 — an unmasked line on PIC2 also needs PIC2's own cascade
/// line (IRQ2 on PIC1) unmasked, not handled here since nothing needs
/// PIC2 yet.
pub fn unmask_irq(irq: u8) {
    let (port, bit) = if irq < 8 {
        (PIC1_DATA, irq)
    } else {
        (PIC2_DATA, irq - 8)
    };
    unsafe {
        let mask = inb(port);
        outb(port, mask & !(1 << bit));
    }
    crate::klog_info!("PIC: unmasked IRQ{}", irq);
}

/// Non-specific End-Of-Interrupt to PIC1 (and PIC2 too, for an IRQ >= 8 —
/// cascaded lines need both PICs acknowledged). Real hardware requirement:
/// without this, the PIC never de-asserts and the SAME IRQ line can't
/// fire again.
pub fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(PIC2_COMMAND, 0x20);
        }
        outb(PIC1_COMMAND, 0x20);
    }
}
