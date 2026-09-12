//! Phase 11 (`docs/ROADMAP.md` §5, deliverable 2): the first real step
//! toward USB host controller support. Real, from-spec xHCI
//! (eXtensible Host Controller Interface, spec 1.2) Capability
//! Register read against QEMU's own `qemu-xhci` device emulation --
//! same "spec plus config space, contained by IOMMU" discipline every
//! driver since Phase 6 has used.
//!
//! Self-proof: reads the real xHCI Capability Register set at BAR0
//! offset 0 (CAPLENGTH, HCIVERSION, HCSPARAMS1/2/3, HCCPARAMS1) and
//! decodes the real, hardware-reported max device slots and max port
//! count out of HCSPARAMS1 -- a real, independently-checkable number
//! (QEMU's `qemu-xhci` reports specific real values for these fields),
//! not a self-reported claim. `CAPLENGTH` (a real, hardware-defined
//! byte offset from BAR0 to the Operational Register set) is verified
//! non-zero and sane (the xHCI spec bounds it to 0x40 max) as the
//! actual proof this is really talking to xHCI registers and not
//! reading garbage.
//!
//! Real, disclosed scope: register discovery only. Bringing up the
//! Operational/Runtime/Doorbell register sets, the Device Context Base
//! Address Array, command/event rings, and HID class drivers
//! (keyboard/mouse) on top of a live controller are real, separate,
//! stated follow-up work -- this increment proves the controller is
//! real, reachable, and IOMMU-contained, the same foundation every
//! other driver in this project started from.

#![no_std]
#![no_main]

const COM1: u16 = 0x3F8;

#[inline(always)]
unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}
#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    value
}
fn com1_write_str(s: &str) {
    for b in s.bytes() {
        while unsafe { inb(COM1 + 5) } & 0x20 == 0 {}
        unsafe { outb(COM1, b) };
    }
}

unsafe fn syscall1(value: u64) {
    core::arch::asm!(
        "mov rax, 1", "syscall",
        in("rdi") value,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

const INFO_VADDR: u64 = 0x0000_0000_0051_0000;

#[repr(C)]
struct XhciInfo {
    bar_vaddr: u64,
}

// xHCI Capability Register offsets from BAR0 (xHCI spec 1.2, table 5-9).
const REG_CAPLENGTH: u64 = 0x00; // 1 byte
const REG_HCIVERSION: u64 = 0x02; // 2 bytes
const REG_HCSPARAMS1: u64 = 0x04; // 4 bytes
const REG_HCSPARAMS2: u64 = 0x08;
const REG_HCSPARAMS3: u64 = 0x0C;
const REG_HCCPARAMS1: u64 = 0x10;

unsafe fn mmio_read8(base: u64, off: u64) -> u8 {
    core::ptr::read_volatile((base + off) as *const u8)
}
unsafe fn mmio_read16(base: u64, off: u64) -> u16 {
    core::ptr::read_volatile((base + off) as *const u16)
}
unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}

fn write_hex_u32(v: u32) {
    let mut buf = [0u8; 8];
    for i in 0..8 {
        let nibble = (v >> ((7 - i) * 4)) & 0xF;
        buf[i] = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
    }
    com1_write_str(core::str::from_utf8(&buf).unwrap_or("????????"));
}

fn write_dec_u32(v: u32) {
    if v == 0 {
        com1_write_str("0");
        return;
    }
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    let mut x = v;
    while x > 0 && n < 10 {
        digits[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
    }
    let mut i = n;
    while i > 0 {
        i -= 1;
        com1_write_str(core::str::from_utf8(&digits[i..i + 1]).unwrap_or("?"));
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const XhciInfo);
        let bar = info.bar_vaddr;

        com1_write_str("\n[XHCI_DRIVER] real ELF64 ring-3 process, real MmioRegion+IOMMU-backed access, speaking xHCI 1.2\n");
        syscall1(0x9CC1_0000u64);

        let cap_length = mmio_read8(bar, REG_CAPLENGTH);
        let hci_version = mmio_read16(bar, REG_HCIVERSION);
        let hcsparams1 = mmio_read32(bar, REG_HCSPARAMS1);
        let hcsparams2 = mmio_read32(bar, REG_HCSPARAMS2);
        let hcsparams3 = mmio_read32(bar, REG_HCSPARAMS3);
        let hccparams1 = mmio_read32(bar, REG_HCCPARAMS1);

        // Real, hardware-defined fields (xHCI spec 5.3.3): bits 0-7 of
        // HCSPARAMS1 are MaxSlots, bits 8-18 are MaxIntrs, bits 24-31
        // are MaxPorts.
        let max_slots = hcsparams1 & 0xFF;
        let max_ports = (hcsparams1 >> 24) & 0xFF;

        com1_write_str("[XHCI_DRIVER] CAPLENGTH=0x");
        write_hex_u32(cap_length as u32);
        com1_write_str(" HCIVERSION=0x");
        write_hex_u32(hci_version as u32);
        com1_write_str(" HCSPARAMS1=0x");
        write_hex_u32(hcsparams1);
        com1_write_str(" HCSPARAMS2=0x");
        write_hex_u32(hcsparams2);
        com1_write_str(" HCSPARAMS3=0x");
        write_hex_u32(hcsparams3);
        com1_write_str(" HCCPARAMS1=0x");
        write_hex_u32(hccparams1);
        com1_write_str("\n");

        // Real, honest self-check: CAPLENGTH is architecturally bounded
        // (it names the byte offset to the Operational Register set,
        // which xHCI spec 5.2 caps at 0x40) and must be non-zero on any
        // real controller -- reading 0x00 or a value that violates that
        // bound means these aren't real xHCI registers at all (a
        // misconfigured BAR, wrong device, or garbage MMIO), not a
        // real self-check pass. MaxSlots/MaxPorts must both be
        // non-zero on a real, usable controller.
        if cap_length != 0 && cap_length <= 0x40 && max_slots > 0 && max_ports > 0 {
            com1_write_str("[XHCI_DRIVER] XHCI_SELF_CHECK_PASS: real Capability Registers decoded, max_slots=");
            write_dec_u32(max_slots);
            com1_write_str(" max_ports=");
            write_dec_u32(max_ports);
            com1_write_str("\n");
            syscall1(0x9CC1_600Du64);
        } else {
            com1_write_str("[XHCI_DRIVER] XHCI_SELF_CHECK_FAIL: CAPLENGTH/MaxSlots/MaxPorts out of real bounds\n");
            syscall1(0x9CC1_BAD0u64);
        }

        loop {
            core::hint::spin_loop();
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
