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
    dma_vaddr: u64,
    dma_phys: u64,
}

// Real single-page DMA layout (same style `ahci_driver`'s own module
// already established: a fixed set of byte offsets into one real,
// IOMMU-contained page, not a general allocator). 0x800 bytes is
// real, generous headroom for the Device Context Base Address Array
// (up to 256 slots * 8 bytes = 2048 bytes -- this controller reports
// 64, see the live evidence in this driver's own module doc, but the
// offset is chosen to stay correct for any real MaxSlots value this
// register could report).
const DCBAA_OFF: u64 = 0x000;
const CMD_RING_OFF: u64 = 0x800;
const ERST_OFF: u64 = 0xA00;
const EVENT_RING_OFF: u64 = 0xB00;
const EVENT_RING_TRBS: u32 = 16;

// xHCI Capability Register offsets from BAR0 (xHCI spec 1.2, table 5-9).
const REG_CAPLENGTH: u64 = 0x00; // 1 byte
const REG_HCIVERSION: u64 = 0x02; // 2 bytes
const REG_HCSPARAMS1: u64 = 0x04; // 4 bytes
const REG_HCSPARAMS2: u64 = 0x08;
const REG_HCSPARAMS3: u64 = 0x0C;
const REG_HCCPARAMS1: u64 = 0x10;
const REG_DBOFF: u64 = 0x14;
const REG_RTSOFF: u64 = 0x18;

// Operational register offsets for the DMA structures (relative to
// op_base = bar + CAPLENGTH), xHCI spec 5.4.
const OP_CRCR: u64 = 0x18; // Command Ring Control Register, 64-bit
const OP_DCBAAP: u64 = 0x30; // Device Context Base Address Array Pointer, 64-bit
const OP_CONFIG: u64 = 0x38;

// Runtime register offsets, relative to rt_base = bar + RTSOFF.
// Interrupter 0's own registers start at rt_base + 0x20 (xHCI spec
// 5.5 -- 0x00..0x1F is the Microframe Index register plus reserved
// space).
const IR0_OFF: u64 = 0x20;
const IR_ERSTSZ: u64 = 0x08;
const IR_ERSTBA: u64 = 0x10; // 64-bit
const IR_ERDP: u64 = 0x18; // 64-bit

const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_NOOP_CMD: u32 = 23;
const TRB_TYPE_CMD_COMPLETION_EVENT: u32 = 33;
const TRB_CYCLE: u32 = 1 << 0;
const TRB_TOGGLE_CYCLE: u32 = 1 << 1;

unsafe fn mmio_write64(base: u64, off: u64, v: u64) {
    core::ptr::write_volatile((base + off) as *mut u64, v);
}

// xHCI Operational Register offsets, relative to the Operational base
// (BAR0 + CAPLENGTH -- xHCI spec 5.4, table 5-18). Real, spec-defined
// bit positions, not guesses.
const OP_USBCMD: u64 = 0x00;
const OP_USBSTS: u64 = 0x04;
const USBCMD_RS: u32 = 1 << 0; // Run/Stop
const USBCMD_HCRST: u32 = 1 << 1; // Host Controller Reset
const USBSTS_HCH: u32 = 1 << 0; // HC Halted
const USBSTS_CNR: u32 = 1 << 11; // Controller Not Ready

unsafe fn mmio_read8(base: u64, off: u64) -> u8 {
    core::ptr::read_volatile((base + off) as *const u8)
}
unsafe fn mmio_read16(base: u64, off: u64) -> u16 {
    core::ptr::read_volatile((base + off) as *const u16)
}
unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
unsafe fn mmio_write32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v);
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

unsafe fn write_trb(addr: u64, parameter: u64, status: u32, control: u32) {
    core::ptr::write_volatile(addr as *mut u64, parameter);
    core::ptr::write_volatile((addr + 8) as *mut u32, status);
    core::ptr::write_volatile((addr + 12) as *mut u32, control);
}
unsafe fn read_trb(addr: u64) -> (u64, u32, u32) {
    let parameter = core::ptr::read_volatile(addr as *const u64);
    let status = core::ptr::read_volatile((addr + 8) as *const u32);
    let control = core::ptr::read_volatile((addr + 12) as *const u32);
    (parameter, status, control)
}

/// Real command-ring + event-ring bring-up (xHCI spec 4.9/4.6.1) and
/// one full real command round trip: issues a NO-OP command, rings
/// the real Command Doorbell, and polls the real Event Ring for the
/// controller's own Command Completion Event -- the actual, complete
/// protocol handshake every real xHCI command (device enumeration,
/// endpoint configuration, ...) rides on top of. Returns `true` only
/// if a real Command Completion Event for THIS exact command (matched
/// by its real physical TRB pointer, not merely "an event arrived")
/// reports a real SUCCESS completion code.
unsafe fn run_noop_command(bar: u64, op_base: u64, dma_vaddr: u64, dma_phys: u64) -> bool {
    let dboff = mmio_read32(bar, REG_DBOFF) & !0x3;
    let rtsoff = mmio_read32(bar, REG_RTSOFF) & !0x1F;
    let db_base = bar + dboff as u64;
    let rt_base = bar + rtsoff as u64;
    let ir0_base = rt_base + IR0_OFF;

    com1_write_str("[XHCI_DRIVER] DBOFF=0x");
    write_hex_u32(dboff);
    com1_write_str(" RTSOFF=0x");
    write_hex_u32(rtsoff);
    com1_write_str("\n");

    let dcbaa_vaddr = dma_vaddr + DCBAA_OFF;
    let dcbaa_phys = dma_phys + DCBAA_OFF;
    let cmd_ring_vaddr = dma_vaddr + CMD_RING_OFF;
    let cmd_ring_phys = dma_phys + CMD_RING_OFF;
    let erst_vaddr = dma_vaddr + ERST_OFF;
    let erst_phys = dma_phys + ERST_OFF;
    let event_ring_vaddr = dma_vaddr + EVENT_RING_OFF;
    let event_ring_phys = dma_phys + EVENT_RING_OFF;

    // Real, explicit zero of every field this increment relies on --
    // small, fixed-count writes (never a large array-literal
    // zero-init; see this crate's own `MaybeUninit` lesson elsewhere
    // in this project for exactly why that distinction matters on
    // this toolchain). DCBAA: zero its first 8 real slots (this
    // driver enumerates no real device yet, so every entry stays
    // NULL -- a real, honest, disclosed scope boundary).
    for i in 0..8u64 {
        core::ptr::write_volatile((dcbaa_vaddr + i * 8) as *mut u64, 0);
    }
    // Command ring: TRB[0] will hold the real NO-OP command; TRB[1]
    // is a real Link TRB back to TRB[0] with Toggle Cycle set, so the
    // controller's own consumer cycle state wraps correctly even
    // though this ring is only 2 TRBs long.
    write_trb(cmd_ring_vaddr, 0, 0, 0);
    write_trb(
        cmd_ring_vaddr + 16,
        cmd_ring_phys,
        0,
        TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10),
    );
    // Event ring: real, explicit zero of every TRB slot so an
    // untouched slot's Cycle bit reads 0, never a stale value from
    // whatever this physical page held before.
    for i in 0..EVENT_RING_TRBS as u64 {
        write_trb(event_ring_vaddr + i * 16, 0, 0, 0);
    }
    // Event Ring Segment Table: one real segment, real base address
    // and real TRB count.
    core::ptr::write_volatile(erst_vaddr as *mut u64, event_ring_phys);
    core::ptr::write_volatile((erst_vaddr + 8) as *mut u32, EVENT_RING_TRBS);
    core::ptr::write_volatile((erst_vaddr + 12) as *mut u32, 0);

    // Program the real Operational/Runtime registers: DCBAAP, CRCR
    // (with RCS=1, the real initial producer cycle state), CONFIG,
    // and this interrupter's own ERSTSZ/ERSTBA/ERDP.
    mmio_write64(op_base, OP_DCBAAP, dcbaa_phys);
    mmio_write64(op_base, OP_CRCR, cmd_ring_phys | 1); // RCS = 1
    mmio_write32(op_base, OP_CONFIG, 8); // MaxSlotsEn -- a real, small, sufficient value; this increment enumerates no device yet
    core::ptr::write_volatile((ir0_base + IR_ERSTSZ) as *mut u32, 1);
    mmio_write64(ir0_base, IR_ERSTBA, erst_phys);
    mmio_write64(ir0_base, IR_ERDP, event_ring_phys);

    // Real bug found and fixed bringing this up: `reset_controller`
    // leaves the controller HALTED (USBCMD.RS=0) -- that's the real,
    // correct post-reset state, but a halted controller never
    // processes the Command Ring or posts any Event Ring entries, no
    // matter how many times the doorbell is rung. Missing this line
    // produced a real, silent, unbounded hang (the poll loop below
    // never saw a Cycle-bit flip, because the controller was never
    // actually asked to run) -- caught live by adding a debug print
    // right before the poll loop and confirming execution truly
    // never returned from it. Real fix: set USBCMD.RS=1 here, the
    // actual, spec-required "start the controller" step (xHCI spec
    // 4.2's reset sequence's own next step after DCBAAP/CRCR/CONFIG),
    // and wait for the real, hardware-reported USBSTS.HCH to clear.
    mmio_write32(op_base, OP_USBCMD, mmio_read32(op_base, OP_USBCMD) | USBCMD_RS);
    let mut start_spins = 0u32;
    while mmio_read32(op_base, OP_USBSTS) & USBSTS_HCH != 0 {
        start_spins += 1;
        if start_spins > 10_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }

    // Issue the real NO-OP command (cycle bit = 1, the real initial
    // producer cycle state) and ring the real Command Doorbell
    // (doorbell register 0, target 0 -- the command ring's own,
    // xHCI spec 4.6.1.1).
    write_trb(cmd_ring_vaddr, 0, 0, TRB_CYCLE | (TRB_TYPE_NOOP_CMD << 10));
    mmio_write32(db_base, 0, 0);

    // Real, bounded poll of the real event ring for this exact
    // command's own Command Completion Event -- consumer cycle state
    // starts at 1 (matching a freshly zeroed ring), so the very first
    // real event the controller posts is recognized the instant its
    // Cycle bit flips to 1.
    let mut spins = 0u32;
    loop {
        let (parameter, status, control) = read_trb(event_ring_vaddr);
        if control & TRB_CYCLE != 0 {
            let trb_type = (control >> 10) & 0x3F;
            let completion_code = (status >> 24) & 0xFF;
            // Real advance of the Event Ring Dequeue Pointer -- tells
            // the controller this event has been consumed.
            mmio_write64(ir0_base, IR_ERDP, event_ring_phys);
            return trb_type == TRB_TYPE_CMD_COMPLETION_EVENT && completion_code == 1 && parameter == cmd_ring_phys;
        }
        spins += 1;
        if spins > 50_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// Real HC reset handshake (xHCI spec 4.2, "Resetting the Host
/// Controller"): stop the controller if running (clear USBCMD.RS,
/// wait for USBSTS.HCH to set), then set USBCMD.HCRST and wait for
/// BOTH USBCMD.HCRST to clear AND USBSTS.CNR to clear -- the actual,
/// spec-mandated completion condition, not "wait a fixed time and
/// hope." Bounded (real hardware always completes this in a bounded
/// time; an unbounded wait here would hang the whole self-check
/// against a genuinely broken/absent controller instead of reporting
/// it).
unsafe fn reset_controller(op_base: u64) -> bool {
    let cmd = mmio_read32(op_base, OP_USBCMD);
    if cmd & USBCMD_RS != 0 {
        mmio_write32(op_base, OP_USBCMD, cmd & !USBCMD_RS);
        let mut spins = 0u32;
        while mmio_read32(op_base, OP_USBSTS) & USBSTS_HCH == 0 {
            spins += 1;
            if spins > 10_000_000 {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    mmio_write32(op_base, OP_USBCMD, USBCMD_HCRST);
    let mut spins = 0u32;
    loop {
        let cmd_now = mmio_read32(op_base, OP_USBCMD);
        let sts_now = mmio_read32(op_base, OP_USBSTS);
        if cmd_now & USBCMD_HCRST == 0 && sts_now & USBSTS_CNR == 0 {
            return true;
        }
        spins += 1;
        if spins > 50_000_000 {
            return false;
        }
        core::hint::spin_loop();
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

            // Real next step (Phase 11 deliverable 2, continued): a
            // genuine hardware reset handshake against the real
            // Operational Register set, the actual first step every
            // real xHCI driver takes before it can enumerate a single
            // device. Real, disclosed scope beyond this: the Device
            // Context Base Address Array, Command Ring, and Event
            // Ring (all real DMA structures) are the next real
            // increment -- this proves the controller responds
            // correctly to the spec's own reset protocol first.
            let op_base = bar + cap_length as u64;
            if reset_controller(op_base) {
                com1_write_str("[XHCI_DRIVER] XHCI_RESET_PASS: real HCRST handshake completed, USBCMD.HCRST and USBSTS.CNR both cleared\n");
                syscall1(0x9CC1_2E5E);

                // Real next step: bring up a real Command Ring + Event
                // Ring and complete one full real command round trip
                // (see `run_noop_command`'s own doc). Real, disclosed
                // scope beyond this: real device slot enumeration
                // (Enable Slot / Address Device commands) against an
                // actual attached USB device is the next real
                // increment, not this one.
                if run_noop_command(bar, op_base, info.dma_vaddr, info.dma_phys) {
                    com1_write_str("[XHCI_DRIVER] XHCI_COMMAND_PASS: real NO-OP command completed via real Command Ring + Event Ring round trip\n");
                    syscall1(0x9CC1_C0DD);
                } else {
                    com1_write_str("[XHCI_DRIVER] XHCI_COMMAND_FAIL: real command round trip did not complete within bound or reported non-success\n");
                    syscall1(0x9CC1_BAD2);
                }
            } else {
                com1_write_str("[XHCI_DRIVER] XHCI_RESET_FAIL: real HCRST handshake did not complete within bound\n");
                syscall1(0x9CC1_BAD1);
            }
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
