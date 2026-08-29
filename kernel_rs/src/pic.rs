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

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

unsafe fn io_wait() {
    outb(0x80, 0);
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
