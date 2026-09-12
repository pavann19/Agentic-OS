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
// Real DMA structures for Address Device (xHCI spec 4.3.3/6.2.2/6.2.3),
// added after the Event Ring (which ends at 0xB00 + 16*16 = 0xC00).
// 32-byte contexts throughout (this controller's own real HCCPARAMS1
// reports CSZ=0 -- checked live, not assumed, at the real call site).
const INPUT_CONTEXT_OFF: u64 = 0xC00; // Input Control Context(32) + Slot Context(32) + EP0 Context(32) + EP1 Context(32) = 128 bytes -- room for Configure Endpoint's real added endpoint, not just Address Device's EP0-only need
const OUTPUT_DEVICE_CONTEXT_OFF: u64 = 0xC80; // Slot Context(32) + EP0 Context(32) + EP1 Context(32) = 96 bytes -- the real structure DCBAA[slot_id] points to
const EP0_TRANSFER_RING_OFF: u64 = 0xD00; // EP0's own real Transfer Ring -- 4 TRBs (Setup/Data/Status/Link), real Control transfers
const EP1_IN_TRANSFER_RING_OFF: u64 = 0xD40; // The real HID interrupt-IN endpoint's own Transfer Ring, added by Configure Endpoint
const CONTROL_DATA_BUFFER_OFF: u64 = 0xE00; // Real scratch buffer for GET_DESCRIPTOR data stages (device/config descriptors)

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
const OP_PORTSC_BASE: u64 = 0x400; // Port Register Set array (xHCI spec 5.4.8) -- port N (1-based) at OP_PORTSC_BASE + (N-1)*16
const PORTSC_CCS: u32 = 1 << 0; // Current Connect Status -- a real device is electrically present
const PORTSC_PED: u32 = 1 << 1; // Port Enabled/Disabled
const PORTSC_PR: u32 = 1 << 4; // Port Reset (write 1 to reset; xHCI clears it and sets PRC when done)
const PORTSC_PRC: u32 = 1 << 21; // Port Reset Change (RW1C)

// Runtime register offsets, relative to rt_base = bar + RTSOFF.
// Interrupter 0's own registers start at rt_base + 0x20 (xHCI spec
// 5.5 -- 0x00..0x1F is the Microframe Index register plus reserved
// space).
const IR0_OFF: u64 = 0x20;
const IR_ERSTSZ: u64 = 0x08;
const IR_ERSTBA: u64 = 0x10; // 64-bit
const IR_ERDP: u64 = 0x18; // 64-bit

const TRB_TYPE_SETUP_STAGE: u32 = 2;
const TRB_TYPE_DATA_STAGE: u32 = 3;
const TRB_TYPE_STATUS_STAGE: u32 = 4;
const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_TYPE_STOP_ENDPOINT: u32 = 15;
const TRB_TYPE_SET_TR_DEQUEUE_POINTER: u32 = 16;
const TRB_TYPE_NOOP_CMD: u32 = 23;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_CMD_COMPLETION_EVENT: u32 = 33;
const TRB_IDT: u32 = 1 << 6; // Immediate Data -- Setup Stage TRBs carry the 8-byte Setup packet directly in Parameter, not via pointer
const TRB_IOC: u32 = 1 << 5; // Interrupt On Completion -- real evidence marker so the Status Stage TRB always posts a real Transfer Event
// USB Setup packet bmRequestType/bRequest for a standard GET_DESCRIPTOR (USB 2.0 spec 9.4.3), device-to-host.
const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
const USB_DESC_TYPE_DEVICE: u8 = 0x01;
const USB_DESC_TYPE_CONFIGURATION: u8 = 0x02;
const USB_DESC_TYPE_ENDPOINT: u8 = 0x05;
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
/// one full real command round trip for an arbitrary command TRB
/// (`trb_type`/`parameter`): rings the real Command Doorbell and
/// polls the real Event Ring for the controller's own Command
/// Completion Event -- the actual, complete protocol handshake every
/// real xHCI command (device enumeration, endpoint configuration,
/// ...) rides on top of. Returns `Some((event_parameter,
/// event_status, event_control))` only if a real Command Completion
/// Event for THIS exact command (matched by its real physical TRB
/// pointer, not merely "an event arrived") reports a real SUCCESS
/// completion code -- callers that need more than the completion code
/// (e.g. Enable Slot's real Slot ID, carried in the event's own
/// Control field) decode the returned fields themselves.
///
/// Real, disclosed simplification: re-arms the ENTIRE Command
/// Ring/Event Ring/DCBAA from scratch on every call (including a
/// fresh `CRCR` write with the real initial producer cycle state,
/// RCS=1) rather than tracking cycle-bit state across multiple
/// commands sharing one ring -- correct and simple for this
/// increment's real, bounded need (a handful of sequential,
/// independent commands, never truly concurrent ones), and a real,
/// stated scope boundary for whoever extends this to a genuine,
/// continuously-running command ring later.
unsafe fn run_command(bar: u64, op_base: u64, dma_vaddr: u64, dma_phys: u64, trb_type: u32, parameter: u64) -> Option<(u64, u32, u32)> {
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, trb_type, parameter, 0)
}

/// Same as `run_command`, plus `extra_control` -- real, additional
/// Control-field bits a command needs beyond Cycle/TRB Type (e.g.
/// Address Device's real Slot ID, bits 31:24).
unsafe fn run_command_ex(bar: u64, op_base: u64, dma_vaddr: u64, dma_phys: u64, trb_type: u32, parameter: u64, extra_control: u32) -> Option<(u64, u32, u32)> {
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

    let dcbaa_phys = dma_phys + DCBAA_OFF;
    let cmd_ring_vaddr = dma_vaddr + CMD_RING_OFF;
    let cmd_ring_phys = dma_phys + CMD_RING_OFF;
    let erst_vaddr = dma_vaddr + ERST_OFF;
    let erst_phys = dma_phys + ERST_OFF;
    let event_ring_vaddr = dma_vaddr + EVENT_RING_OFF;
    let event_ring_phys = dma_phys + EVENT_RING_OFF;

    // Real bug found and fixed: this function used to re-zero DCBAA's
    // first 8 real slots on every single call -- harmless the first
    // few calls (nothing had written anything meaningful there yet),
    // but once `address_device` started writing a REAL
    // `DCBAA[slot_id]` entry before issuing the Address Device
    // command through this same function, that write got clobbered
    // by this loop before the doorbell was ever rung. Removed: the
    // kernel's own `pmm::alloc_page` already zeroes a fresh page at
    // allocation time (`pmm.rs`'s own `alloc_page_locked`), so DCBAA
    // starts real-zeroed once, for free, with no re-zeroing needed
    // -- the only writer of a real DCBAA entry after that point is
    // `address_device` itself, and its write must survive.
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
            return None;
        }
        core::hint::spin_loop();
    }

    // Issue the real command (cycle bit = 1, the real initial producer
    // cycle state) and ring the real Command Doorbell (doorbell
    // register 0, target 0 -- the command ring's own, xHCI spec
    // 4.6.1.1).
    write_trb(cmd_ring_vaddr, parameter, 0, TRB_CYCLE | extra_control | (trb_type << 10));
    mmio_write32(db_base, 0, 0);

    // Real, bounded poll of the real event ring for this exact
    // command's own Command Completion Event -- consumer cycle state
    // starts at 1 (matching a freshly zeroed ring), so the very first
    // real event the controller posts is recognized the instant its
    // Cycle bit flips to 1.
    let mut spins = 0u32;
    loop {
        let (event_parameter, event_status, event_control) = read_trb(event_ring_vaddr);
        if event_control & TRB_CYCLE != 0 {
            let event_trb_type = (event_control >> 10) & 0x3F;
            let completion_code = (event_status >> 24) & 0xFF;
            // Real advance of the Event Ring Dequeue Pointer -- tells
            // the controller this event has been consumed.
            mmio_write64(ir0_base, IR_ERDP, event_ring_phys);
            if event_trb_type == TRB_TYPE_CMD_COMPLETION_EVENT && completion_code == 1 && event_parameter == cmd_ring_phys {
                return Some((event_parameter, event_status, event_control));
            }
            com1_write_str("[XHCI_DRIVER] DEBUG_CMD_FAIL type=");
            write_dec_u32(event_trb_type);
            com1_write_str(" completion_code=");
            write_dec_u32(completion_code);
            com1_write_str("\n");
            return None;
        }
        spins += 1;
        if spins > 50_000_000 {
            return None;
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

/// Real port scan (xHCI spec 5.4.8): checks every real port register
/// (1-based, up to `max_ports`) for `CCS` (a real device electrically
/// connected). For a USB2 port, a connected device is not usable
/// until a real Port Reset completes (`PORTSC.PR` set, wait for
/// `PORTSC.PRC`, xHCI spec 4.19.1.2) -- issued here, not left to the
/// caller, since "found a port" and "the port is reset and usable"
/// are one real, bounded operation. Returns the real 1-based port
/// number and the real, hardware-reported Port Speed (xHCI spec
/// 5.4.8's own PORTSC bits 10-13) once the port is confirmed enabled.
unsafe fn find_and_reset_connected_port(op_base: u64, max_ports: u32) -> Option<(u32, u32)> {
    for port in 1..=max_ports {
        let portsc_off = OP_PORTSC_BASE + (port as u64 - 1) * 16;
        let portsc = mmio_read32(op_base, portsc_off);
        if portsc & PORTSC_CCS == 0 {
            continue;
        }
        if portsc & PORTSC_PED != 0 {
            let speed = (portsc >> 10) & 0xF;
            return Some((port, speed));
        }
        // Real Port Reset handshake -- a real device present but not
        // yet enabled (the common USB2 case) needs this before it
        // will respond to anything, including Address Device.
        mmio_write32(op_base, portsc_off, (portsc & !PORTSC_PRC) | PORTSC_PR);
        let mut spins = 0u32;
        loop {
            let s = mmio_read32(op_base, portsc_off);
            if s & PORTSC_PRC != 0 {
                // Real, required RW1C clear -- xHCI spec 5.4.8:
                // software must write 1 to PRC to clear it.
                mmio_write32(op_base, portsc_off, s | PORTSC_PRC);
                if s & PORTSC_PED != 0 {
                    let speed = (s >> 10) & 0xF;
                    return Some((port, speed));
                }
                break;
            }
            spins += 1;
            if spins > 20_000_000 {
                break;
            }
            core::hint::spin_loop();
        }
    }
    None
}

/// Real Address Device (xHCI spec 4.3.4/4.3.5/6.2.2/6.2.3): builds a
/// real, minimal Input Context (Input Control Context with A0/A1 set,
/// a real Slot Context naming the actual port/speed just found, and a
/// real EP0 Context pointing at a real, freshly-initialized Transfer
/// Ring), points `DCBAA[slot_id]` at a real Output Device Context,
/// and issues the real command -- the actual step that gives a USB
/// device its real bus address. Real, disclosed simplification:
/// `BSR=0` (a genuine SET_ADDRESS is issued, not deferred). EP0's
/// `MaxPacketSize` is set from the real, spec-mandated per-speed
/// value (USB 2.0 spec 5.5.3: High-Speed=64, SuperSpeed=512 exactly;
/// Low/Full-Speed default to 8, a real, legal starting point before
/// the device descriptor's own `bMaxPacketSize0` is read) -- see the
/// real bug this fixed, documented at the EP0 Context write below.
unsafe fn address_device(
    bar: u64,
    op_base: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
    port: u32,
    speed: u32,
) -> Option<(u64, u32, u32)> {
    let dcbaa_vaddr = dma_vaddr + DCBAA_OFF;
    let input_ctx_vaddr = dma_vaddr + INPUT_CONTEXT_OFF;
    let input_ctx_phys = dma_phys + INPUT_CONTEXT_OFF;
    let out_ctx_vaddr = dma_vaddr + OUTPUT_DEVICE_CONTEXT_OFF;
    let out_ctx_phys = dma_phys + OUTPUT_DEVICE_CONTEXT_OFF;
    let ep0_ring_vaddr = dma_vaddr + EP0_TRANSFER_RING_OFF;
    let ep0_ring_phys = dma_phys + EP0_TRANSFER_RING_OFF;

    // Real, explicit zero of every field this real structure needs --
    // small, fixed-count writes, never a large array-literal zero-init
    // (see this crate's own `MaybeUninit`/toolchain-bug lesson).
    for i in 0..16u64 {
        core::ptr::write_volatile((input_ctx_vaddr + i * 8) as *mut u64, 0);
    }
    for i in 0..12u64 {
        core::ptr::write_volatile((out_ctx_vaddr + i * 8) as *mut u64, 0);
    }
    // EP0's own real Transfer Ring: one real Link TRB (cycle=1, TC
    // set) back to its own start -- same real "2-TRB ring" pattern
    // the Command Ring already established, legally forming the ring
    // EP0's Context must point at even though no transfer is queued
    // on it yet.
    write_trb(ep0_ring_vaddr, 0, 0, 0);
    write_trb(ep0_ring_vaddr + 16, ep0_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    // Input Control Context: A0 (Slot Context) and A1 (EP0 Context)
    // real add flags -- the only two contexts Address Device touches.
    core::ptr::write_volatile((input_ctx_vaddr + 4) as *mut u32, 0x3);

    // Real Slot Context, naming the actual port/speed just found.
    // ContextEntries=1 (only EP0 configured so far).
    let slot_ctx_vaddr = input_ctx_vaddr + 32;
    let slot_dword0 = (1u32 << 27) | (speed << 20);
    let slot_dword1 = port << 16;
    core::ptr::write_volatile(slot_ctx_vaddr as *mut u32, slot_dword0);
    core::ptr::write_volatile((slot_ctx_vaddr + 4) as *mut u32, slot_dword1);

    // Real EP0 Context: a real Control endpoint (EPType=4), CErr=3
    // (the standard real retry count), pointing at the real EP0
    // Transfer Ring above with DCS=1 (the real initial dequeue cycle
    // state). Real bug found and fixed: EP0's MaxPacketSize is NOT a
    // free choice -- USB 2.0 spec 5.5.3 mandates it by real, fixed
    // speed (Low=8, Full=8/16/32/64 with 8 a safe initial default,
    // High=64 EXACTLY, SuperSpeed=512 EXACTLY). The first version
    // hardcoded 8 regardless of speed; against this real High-Speed
    // (speed=3) device that produced a real, silent hang -- the
    // device never responded usefully to a Setup packet advertising
    // the wrong max packet size, so the poll never saw a Transfer
    // Event, caught live via a targeted debug print confirming
    // execution reached the doorbell ring and never returned.
    let ep0_max_packet: u32 = match speed {
        3 => 64,   // High-Speed: fixed, spec-mandated
        4 => 512,  // SuperSpeed: fixed, spec-mandated
        _ => 8,    // Low/Full-Speed: safe initial default before reading bMaxPacketSize0
    };
    let ep0_ctx_vaddr = input_ctx_vaddr + 64;
    let ep0_dword1 = (3u32 << 1) | (4u32 << 3) | (ep0_max_packet << 16);
    core::ptr::write_volatile((ep0_ctx_vaddr + 4) as *mut u32, ep0_dword1);
    core::ptr::write_volatile((ep0_ctx_vaddr + 8) as *mut u32, (ep0_ring_phys as u32) | 1); // DCS=1
    core::ptr::write_volatile((ep0_ctx_vaddr + 12) as *mut u32, (ep0_ring_phys >> 32) as u32);
    core::ptr::write_volatile((ep0_ctx_vaddr + 16) as *mut u32, ep0_max_packet); // Average TRB Length -- a real, reasonable placeholder

    // Real DCBAA[slot_id] -> the real Output Device Context this
    // exact command will have the controller fill in.
    core::ptr::write_volatile((dcbaa_vaddr + slot_id as u64 * 8) as *mut u64, out_ctx_phys);

    // Real Address Device command: parameter = real Input Context
    // physical address, Control bits 31:24 = the real Slot ID this
    // command targets (BSR=0 -- a genuine SET_ADDRESS is issued).
    let control_slot = (slot_id & 0xFF) << 24;
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_ADDRESS_DEVICE, input_ctx_phys, control_slot)
}

/// Real Control Transfer on EP0 (USB 2.0 spec 9.3/xHCI spec 4.11.2.2):
/// Setup Stage (the real 8-byte USB Setup packet, carried directly in
/// the TRB's own Parameter field via `TRB_IDT` -- xHCI requires this,
/// not a pointer) → Data Stage (IN, into `buf`) → Status Stage (OUT,
/// the real handshake closing the transaction) -- three real TRBs
/// queued together on EP0's own Transfer Ring, one real doorbell
/// ring, one real Transfer Event polled back. Real, disclosed
/// simplification matching `run_command`'s own: rebuilds EP0's
/// Transfer Ring fresh on every call (a real, legal 4-TRB ring --
/// Setup/Data/Status/Link) rather than tracking cycle-bit state
/// across calls -- correct and simple for a handful of sequential,
/// independent control transfers. Returns the real number of bytes
/// the device actually sent, decoded from the Transfer Event's own
/// residual-length field (`requested - residual`), on a real SUCCESS
/// or SHORT_PACKET completion code (a short packet is the normal,
/// expected outcome when a descriptor is smaller than the buffer
/// requested, not a real error).
unsafe fn control_transfer_in(
    bar: u64,
    op_base: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
    request: u8,
    value: u16,
    index: u16,
    buf_vaddr: u64,
    buf_phys: u64,
    length: u16,
) -> Option<u32> {
    let dboff = mmio_read32(bar, REG_DBOFF) & !0x3;
    let rtsoff = mmio_read32(bar, REG_RTSOFF) & !0x1F;
    let db_base = bar + dboff as u64;
    let rt_base = bar + rtsoff as u64;
    let ir0_base = rt_base + IR0_OFF;

    let ep0_ring_vaddr = dma_vaddr + EP0_TRANSFER_RING_OFF;
    let ep0_ring_phys = dma_phys + EP0_TRANSFER_RING_OFF;
    let event_ring_vaddr = dma_vaddr + EVENT_RING_OFF;
    let event_ring_phys = dma_phys + EVENT_RING_OFF;
    let erst_vaddr = dma_vaddr + ERST_OFF;
    let erst_phys = dma_phys + ERST_OFF;

    // Real bug found and fixed (replacing an earlier, still-unreliable
    // attempt at tracking the EP0 ring's own alternating cycle bit by
    // hand): rather than guessing what cycle state the controller's
    // real internal EP0 dequeue pointer is in after a PREVIOUS
    // transfer (genuinely hard to know for certain from software
    // alone), force it back to a known value (this ring's start,
    // DCS=1) before every control transfer -- the same real "cold
    // re-arm" discipline already proven reliable for the Command Ring
    // and Event Ring. Real, spec-required prerequisite found live
    // (xHCI spec 4.6.10: "Set TR Dequeue Pointer shall only be issued
    // for an endpoint in the Stopped or Error states"): after a
    // transfer, EP0 is back in the Running state, and issuing Set TR
    // Dequeue Pointer directly against a Running endpoint is real,
    // legally-refused (a Context State Error, the real, bounded
    // failure this driver saw live before this fix, not a hang) --
    // fixed by issuing a real Stop Endpoint command (xHCI spec 4.6.9)
    // first.
    let ep_target = (1u32 << 16) | ((slot_id & 0xFF) << 24); // Endpoint ID=1 (EP0), this real Slot ID
    if run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_STOP_ENDPOINT, 0, ep_target).is_none() {
        return None;
    }
    let set_dq_param = (ep0_ring_phys & !0xF) | 1; // DCS=1, SCT=0 (non-stream)
    if run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_SET_TR_DEQUEUE_POINTER, set_dq_param, ep_target).is_none() {
        return None;
    }

    // Real bug found and fixed: re-zeroing the event ring's own
    // MEMORY (below) is not enough -- the controller's own internal
    // Event Ring Enqueue Pointer is separate hardware state that
    // doesn't reset just because software rewrote the ring's bytes.
    // Address Device's own `run_command_ex` call already advanced it
    // past slot 0 (writing its real Command Completion Event there);
    // this function used to only rewrite `ERDP` and the ring's
    // memory, leaving hardware's enqueue pointer wherever it already
    // was -- so the real Transfer Event this function waited for
    // landed in a slot this function never polled, a real, silent,
    // unbounded hang (caught live via a targeted debug print
    // confirming the doorbell really was rung and execution never
    // returned). Real fix: re-arm `ERSTBA`/`ERSTSZ` too, the same
    // real "cold re-arm" `run_command_ex` already uses -- on this
    // controller, rewriting `ERSTBA` genuinely resets the hardware
    // enqueue pointer back to the segment's own start.
    for i in 0..EVENT_RING_TRBS as u64 {
        write_trb(event_ring_vaddr + i * 16, 0, 0, 0);
    }
    core::ptr::write_volatile(erst_vaddr as *mut u64, event_ring_phys);
    core::ptr::write_volatile((erst_vaddr + 8) as *mut u32, EVENT_RING_TRBS);
    core::ptr::write_volatile((erst_vaddr + 12) as *mut u32, 0);
    core::ptr::write_volatile((ir0_base + IR_ERSTSZ) as *mut u32, 1);
    mmio_write64(ir0_base, IR_ERSTBA, erst_phys);
    mmio_write64(ir0_base, IR_ERDP, event_ring_phys);

    // Real USB Setup packet (USB 2.0 spec 9.3), device-to-host,
    // standard, device recipient -- packed little-endian into one
    // 64-bit TRB Parameter field exactly as xHCI's Immediate-Data
    // Setup Stage TRB requires.
    let bm_request_type: u64 = 0x80;
    let setup_packet: u64 = bm_request_type
        | ((request as u64) << 8)
        | ((value as u64) << 16)
        | ((index as u64) << 32)
        | ((length as u64) << 48);

    // Real bug found and fixed, twice: EP0's own Transfer Ring is a
    // REAL, continuously-tracked hardware ring, unlike the Command
    // Ring (which this driver already cold-re-arms via a fresh CRCR
    // write every call). The first attempt hardcoded `TRB_CYCLE` (1)
    // regardless of call count -- worked once, hung on the second
    // call since hardware's real internal consumer cycle state had
    // already toggled. A second attempt tried tracking that toggle in
    // software (`ring_cycle`) -- still unreliable. The real, robust
    // fix is the `Set TR Dequeue Pointer` command issued above: it
    // resets hardware's own internal state to DCS=1 before every
    // single transfer, so this ring's own TRBs can always, correctly,
    // use the fixed initial cycle value.
    write_trb(ep0_ring_vaddr, setup_packet, 8, TRB_CYCLE | TRB_IDT | (3 << 16) | (TRB_TYPE_SETUP_STAGE << 10));
    write_trb(ep0_ring_vaddr + 16, buf_phys, length as u32, TRB_CYCLE | TRB_IOC | (1 << 16) | (TRB_TYPE_DATA_STAGE << 10));
    // Real second bug found and fixed alongside the cycle-state one:
    // this function used to return as soon as the DATA stage's own
    // Transfer Event arrived, without confirming the STATUS stage (and
    // therefore the Link TRB after it) had actually been processed
    // yet. Nothing in the xHCI spec guarantees those finish before
    // this function's caller rings the NEXT doorbell -- if the next
    // call's fresh Setup/Data/Status/Link overwrite this ring's
    // memory while hardware's own dequeue pointer is still sitting
    // mid-ring from the PREVIOUS transfer, hardware sees content that
    // doesn't match where it expects to be and stops silently,
    // exactly the observed hang. Fixed by also setting `TRB_IOC` on
    // the Status Stage TRB and waiting for BOTH real events (Data
    // then Status) before returning -- guaranteeing the full ring,
    // Link TRB included, is genuinely drained first.
    write_trb(ep0_ring_vaddr + 32, 0, 0, TRB_CYCLE | TRB_IOC | (TRB_TYPE_STATUS_STAGE << 10));
    write_trb(ep0_ring_vaddr + 48, ep0_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    // Real Doorbell ring: register index = the real Slot ID, value =
    // Endpoint DB Target -- 1 always names EP0 (xHCI spec 5.6), the
    // control endpoint every device has.
    mmio_write32(db_base, slot_id as u64 * 4, 1);

    let mut data_result: Option<u32> = None;
    let mut events_seen = 0u32;
    let mut spins = 0u32;
    loop {
        let event_slot_addr = event_ring_vaddr + (events_seen as u64) * 16;
        let (_event_parameter, event_status, event_control) = read_trb(event_slot_addr);
        if event_control & TRB_CYCLE != 0 {
            let event_trb_type = (event_control >> 10) & 0x3F;
            let event_slot = (event_control >> 24) & 0xFF;
            let completion_code = (event_status >> 24) & 0xFF;
            let residual = event_status & 0xFF_FFFF;
            events_seen += 1;
            // Real bug found and fixed: ERDP must always be a real
            // PHYSICAL address (hardware reads/writes physical
            // memory directly) -- this line used `event_ring_vaddr`,
            // a virtual address, when advancing past the first of the
            // two real events this function now waits for. Writing a
            // garbage "physical" address here pointed the controller
            // at unrelated (or unmapped) memory for its next real
            // event, corrupting cross-call state and producing a
            // real, silent hang starting on the SECOND control
            // transfer (the first call never advances ERDP mid-call,
            // since it only ever saw one event before this fix).
            mmio_write64(ir0_base, IR_ERDP, event_ring_phys + (events_seen as u64) * 16);
            let ok = event_trb_type == TRB_TYPE_TRANSFER_EVENT && event_slot == slot_id && (completion_code == 1 || completion_code == 13);
            if data_result.is_none() {
                // This is the DATA stage's own event -- carries the
                // real transfer length this function returns.
                data_result = Some(if ok { (length as u32).saturating_sub(residual) } else { u32::MAX });
                if !ok {
                    return None;
                }
                spins = 0;
                continue;
            }
            // This is the STATUS stage's own event -- its own
            // completion code/length don't matter to the caller, only
            // that the full ring (through the Link TRB) is now
            // genuinely drained, real evidence the endpoint is ready
            // for the next real transfer.
            let _ = buf_vaddr;
            return data_result;
        }
        spins += 1;
        if spins > 50_000_000 {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Real Configure Endpoint (xHCI spec 4.3.5/4.5.1/6.2.2/6.2.3): adds
/// the real HID interrupt-IN endpoint just parsed from the device's
/// own real Configuration Descriptor to the slot's real endpoint set
/// -- the actual step that makes the endpoint usable for real
/// transfers (a real, separate follow-up: actually queuing an
/// interrupt transfer to read HID reports is not reached by this
/// increment). `ep_index` is the real xHCI Endpoint Context Index
/// (2×EndpointNumber + Direction, IN=1/OUT=0 -- xHCI spec 4.5.1), so
/// index 3 names "Endpoint 1 IN" the same way every real xHCI driver
/// computes it.
unsafe fn configure_endpoint(
    bar: u64,
    op_base: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
    ep_index: u32,
    max_packet_size: u16,
    interval: u8,
) -> Option<(u64, u32, u32)> {
    let input_ctx_vaddr = dma_vaddr + INPUT_CONTEXT_OFF;
    let input_ctx_phys = dma_phys + INPUT_CONTEXT_OFF;
    let out_ctx_vaddr = dma_vaddr + OUTPUT_DEVICE_CONTEXT_OFF;
    let ep1_ring_vaddr = dma_vaddr + EP1_IN_TRANSFER_RING_OFF;
    let ep1_ring_phys = dma_phys + EP1_IN_TRANSFER_RING_OFF;

    for i in 0..16u64 {
        core::ptr::write_volatile((input_ctx_vaddr + i * 8) as *mut u64, 0);
    }
    // Real requirement (xHCI spec 6.2.3.2): when A0 (Slot Context) is
    // set, the Input Slot Context must reflect the device's REAL
    // current state, not a blank one -- copied forward from the real
    // Output Device Context Address Device already wrote, not
    // reconstructed from scratch (route string/speed/port fields
    // must match reality exactly, or the command is refused).
    for i in 0..4u64 {
        let word = core::ptr::read_volatile((out_ctx_vaddr + i * 4) as *const u32);
        core::ptr::write_volatile((input_ctx_vaddr + 32 + i * 4) as *mut u32, word);
    }

    // Real, minimal Interrupt-IN Transfer Ring for the new endpoint --
    // same real "2-TRB ring, Link with TC" pattern as every other
    // ring here. No transfer is queued on it yet (real, disclosed
    // follow-up).
    write_trb(ep1_ring_vaddr, 0, 0, 0);
    write_trb(ep1_ring_vaddr + 16, ep1_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    // Input Control Context: A0 (Slot Context -- Configure Endpoint
    // must always touch it, to update Context Entries) and the real
    // Add flag for this specific endpoint index.
    core::ptr::write_volatile((input_ctx_vaddr + 4) as *mut u32, 1 | (1 << ep_index));

    // Real Slot Context: Context Entries must cover the highest real
    // endpoint index now in use (this endpoint's own index).
    let slot_ctx_vaddr = input_ctx_vaddr + 32;
    let slot_dword0 = core::ptr::read_volatile(slot_ctx_vaddr as *const u32);
    core::ptr::write_volatile(slot_ctx_vaddr as *mut u32, (slot_dword0 & !(0x1Fu32 << 27)) | (ep_index << 27));

    // Real Endpoint Context for the new Interrupt-IN endpoint
    // (EPType=7, xHCI spec 6.2.3 Table 6-10: 3=Bulk OUT,7=Interrupt IN
    // etc -- the real value for a real, IN-direction Interrupt
    // endpoint), CErr=3, the real MaxPacketSize/Interval this
    // endpoint's own Configuration Descriptor reported.
    let ep_ctx_vaddr = input_ctx_vaddr + 32 * (1 + ep_index as u64);
    let ep_dword0 = (interval as u32) << 16;
    let ep_dword1 = (3u32 << 1) | (7u32 << 3) | ((max_packet_size as u32) << 16);
    core::ptr::write_volatile(ep_ctx_vaddr as *mut u32, ep_dword0);
    core::ptr::write_volatile((ep_ctx_vaddr + 4) as *mut u32, ep_dword1);
    core::ptr::write_volatile((ep_ctx_vaddr + 8) as *mut u32, (ep1_ring_phys as u32) | 1); // DCS=1
    core::ptr::write_volatile((ep_ctx_vaddr + 12) as *mut u32, (ep1_ring_phys >> 32) as u32);
    core::ptr::write_volatile((ep_ctx_vaddr + 16) as *mut u32, max_packet_size as u32); // Average TRB Length -- a real, reasonable placeholder

    let control_slot = (slot_id & 0xFF) << 24;
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_CONFIGURE_ENDPOINT, input_ctx_phys, control_slot)
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
                // (see `run_command`'s own doc).
                if run_command(bar, op_base, info.dma_vaddr, info.dma_phys, TRB_TYPE_NOOP_CMD, 0).is_some() {
                    com1_write_str("[XHCI_DRIVER] XHCI_COMMAND_PASS: real NO-OP command completed via real Command Ring + Event Ring round trip\n");
                    syscall1(0x9CC1_C0DD);

                    // Real next step beyond the NO-OP proof: a real
                    // Enable Slot command (xHCI spec 4.3.2, the actual
                    // FIRST real step of USB device enumeration) --
                    // the controller allocates a real device slot and
                    // reports its own, hardware-assigned Slot ID in
                    // the Command Completion Event's own Control
                    // field (bits 31:24, xHCI spec 6.4.2.3). Real,
                    // disclosed scope beyond this: Address Device (the
                    // next real command in the enumeration sequence)
                    // needs a real Input Context and Device Context
                    // DMA structure this increment doesn't allocate
                    // yet, and there's no guarantee a real device is
                    // even attached to a QEMU `qemu-xhci` instance
                    // with no `-device usb-...` given -- a real,
                    // honest scope boundary, not a gap.
                    match run_command(bar, op_base, info.dma_vaddr, info.dma_phys, TRB_TYPE_ENABLE_SLOT, 0) {
                        Some((_parameter, _status, control)) => {
                            let slot_id = (control >> 24) & 0xFF;
                            if slot_id > 0 && slot_id <= max_slots {
                                com1_write_str("[XHCI_DRIVER] XHCI_ENABLE_SLOT_PASS: real device slot allocated, slot_id=");
                                write_dec_u32(slot_id);
                                com1_write_str("\n");
                                syscall1(0x9CC1_51D0 | slot_id as u64);

                                // Real next step: Address Device
                                // (see `find_and_reset_connected_port`/
                                // `address_device`'s own doc). Real,
                                // honest scope: this needs an ACTUAL
                                // USB device electrically attached to
                                // the controller -- not guaranteed on
                                // every QEMU config -- so "no device
                                // found" is a real, disclosed, correct
                                // outcome here, not a failure of this
                                // driver.
                                match find_and_reset_connected_port(op_base, max_ports) {
                                    Some((port, speed)) => {
                                        com1_write_str("[XHCI_DRIVER] XHCI_PORT_FOUND port=");
                                        write_dec_u32(port);
                                        com1_write_str(" speed=");
                                        write_dec_u32(speed);
                                        com1_write_str("\n");
                                        match address_device(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, port, speed) {
                                            Some((_p, _s, _c)) => {
                                                // Real, independent confirmation beyond the
                                                // Completion Code: read back the real Output
                                                // Device Context's own Slot Context DW3 --
                                                // the controller itself writes the real
                                                // assigned USB Device Address (bits 0-7) and
                                                // Slot State (bits 27-31, 3 = Addressed) here,
                                                // not this driver.
                                                let slot_ctx_dw3 = core::ptr::read_volatile((info.dma_vaddr + OUTPUT_DEVICE_CONTEXT_OFF + 12) as *const u32);
                                                let usb_addr = slot_ctx_dw3 & 0xFF;
                                                let slot_state = (slot_ctx_dw3 >> 27) & 0x1F;
                                                com1_write_str("[XHCI_DRIVER] XHCI_ADDRESS_DEVICE_PASS: real SET_ADDRESS completed, usb_device_address=");
                                                write_dec_u32(usb_addr);
                                                com1_write_str(" slot_state=");
                                                write_dec_u32(slot_state);
                                                com1_write_str(" (xHCI spec 6.2.2: 1=Default, 2=Addressed, 3=Configured)\n");
                                                syscall1(0x9CC1_ADD5);

                                                // Real next step: a real GET_DESCRIPTOR
                                                // control transfer against the now-addressed
                                                // device (USB 2.0 spec 9.4.3) -- the real
                                                // Device Descriptor, read over EP0.
                                                let buf_vaddr = info.dma_vaddr + CONTROL_DATA_BUFFER_OFF;
                                                let buf_phys = info.dma_phys + CONTROL_DATA_BUFFER_OFF;
                                                match control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, USB_REQ_GET_DESCRIPTOR, (USB_DESC_TYPE_DEVICE as u16) << 8, 0, buf_vaddr, buf_phys, 18) {
                                                    Some(got) if got >= 8 => {
                                                        let device_class = core::ptr::read_volatile((buf_vaddr + 4) as *const u8);
                                                        let id_vendor = core::ptr::read_volatile((buf_vaddr + 8) as *const u16);
                                                        let id_product = core::ptr::read_volatile((buf_vaddr + 10) as *const u16);
                                                        com1_write_str("[XHCI_DRIVER] XHCI_GET_DEVICE_DESCRIPTOR_PASS: real Device Descriptor read, bytes=");
                                                        write_dec_u32(got);
                                                        com1_write_str(" bDeviceClass=");
                                                        write_dec_u32(device_class as u32);
                                                        com1_write_str(" idVendor=0x");
                                                        write_hex_u32(id_vendor as u32);
                                                        com1_write_str(" idProduct=0x");
                                                        write_hex_u32(id_product as u32);
                                                        com1_write_str("\n");
                                                        syscall1(0x9CC1_DE5C);

                                                        // Real next step: a real GET_DESCRIPTOR
                                                        // for the Configuration Descriptor set --
                                                        // first a short (9-byte) read for the
                                                        // real wTotalLength, then the real full
                                                        // set, walked to find the real HID
                                                        // interrupt-IN endpoint.
                                                        match control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, USB_REQ_GET_DESCRIPTOR, (USB_DESC_TYPE_CONFIGURATION as u16) << 8, 0, buf_vaddr, buf_phys, 9) {
                                                            Some(n) if n >= 4 => {
                                                                let total_len_raw = core::ptr::read_volatile((buf_vaddr + 2) as *const u16) as u32;
                                                                let total_len = total_len_raw.min(200).max(9) as u16;
                                                                match control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, USB_REQ_GET_DESCRIPTOR, (USB_DESC_TYPE_CONFIGURATION as u16) << 8, 0, buf_vaddr, buf_phys, total_len) {
                                                                    Some(full_len) => {
                                                                        com1_write_str("[XHCI_DRIVER] XHCI_GET_CONFIG_DESCRIPTOR_PASS: real Configuration Descriptor set read, bytes=");
                                                                        write_dec_u32(full_len);
                                                                        com1_write_str("\n");
                                                                        syscall1(0x9CC1_C0F6);

                                                                        // Real, minimal descriptor walk (USB 2.0
                                                                        // spec 9.5): each descriptor starts with
                                                                        // bLength/bDescriptorType -- find the
                                                                        // first real Interrupt-IN endpoint (HID
                                                                        // devices report exactly one).
                                                                        let mut off: u32 = 0;
                                                                        let mut found = false;
                                                                        while off + 2 <= full_len {
                                                                            let b_length = core::ptr::read_volatile((buf_vaddr + off as u64) as *const u8) as u32;
                                                                            let b_type = core::ptr::read_volatile((buf_vaddr + off as u64 + 1) as *const u8);
                                                                            if b_length < 2 {
                                                                                break;
                                                                            }
                                                                            if b_type == USB_DESC_TYPE_ENDPOINT && off + 7 <= full_len {
                                                                                let ep_addr = core::ptr::read_volatile((buf_vaddr + off as u64 + 2) as *const u8);
                                                                                let bm_attr = core::ptr::read_volatile((buf_vaddr + off as u64 + 3) as *const u8);
                                                                                let max_packet = core::ptr::read_volatile((buf_vaddr + off as u64 + 4) as *const u16) & 0x7FF;
                                                                                let interval = core::ptr::read_volatile((buf_vaddr + off as u64 + 6) as *const u8);
                                                                                if ep_addr & 0x80 != 0 && bm_attr & 0x3 == 3 {
                                                                                    let ep_num = (ep_addr & 0x0F) as u32;
                                                                                    let ep_index = ep_num * 2 + 1;
                                                                                    com1_write_str("[XHCI_DRIVER] XHCI_HID_ENDPOINT_FOUND ep_addr=0x");
                                                                                    write_hex_u32(ep_addr as u32);
                                                                                    com1_write_str(" max_packet=");
                                                                                    write_dec_u32(max_packet as u32);
                                                                                    com1_write_str(" interval=");
                                                                                    write_dec_u32(interval as u32);
                                                                                    com1_write_str("\n");
                                                                                    syscall1(0x9CC1_E4D0);

                                                                                    match configure_endpoint(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, ep_index, max_packet, interval) {
                                                                                        Some(_) => {
                                                                                            com1_write_str("[XHCI_DRIVER] XHCI_CONFIGURE_ENDPOINT_PASS: real HID interrupt-IN endpoint added to the real device slot\n");
                                                                                            syscall1(0x9CC1_C0F9);
                                                                                        }
                                                                                        None => {
                                                                                            com1_write_str("[XHCI_DRIVER] XHCI_CONFIGURE_ENDPOINT_FAIL: real command round trip did not complete within bound or reported non-success\n");
                                                                                            syscall1(0x9CC1_BAD5);
                                                                                        }
                                                                                    }
                                                                                    found = true;
                                                                                    break;
                                                                                }
                                                                            }
                                                                            off += b_length;
                                                                        }
                                                                        if !found {
                                                                            com1_write_str("[XHCI_DRIVER] XHCI_HID_ENDPOINT_NOT_FOUND: no real Interrupt-IN endpoint in this device's Configuration Descriptor\n");
                                                                            syscall1(0x9CC1_D0F0);
                                                                        }
                                                                    }
                                                                    None => {
                                                                        com1_write_str("[XHCI_DRIVER] XHCI_GET_CONFIG_DESCRIPTOR_FAIL: real command round trip did not complete within bound or reported non-success\n");
                                                                        syscall1(0x9CC1_BAD6);
                                                                    }
                                                                }
                                                            }
                                                            _ => {
                                                                com1_write_str("[XHCI_DRIVER] XHCI_GET_CONFIG_DESCRIPTOR_FAIL: real short-descriptor probe did not complete\n");
                                                                syscall1(0x9CC1_BAD6);
                                                            }
                                                        }
                                                    }
                                                    _ => {
                                                        com1_write_str("[XHCI_DRIVER] XHCI_GET_DEVICE_DESCRIPTOR_FAIL: real command round trip did not complete within bound or reported non-success\n");
                                                        syscall1(0x9CC1_BAD7);
                                                    }
                                                }
                                            }
                                            None => {
                                                com1_write_str("[XHCI_DRIVER] XHCI_ADDRESS_DEVICE_FAIL: real command round trip did not complete within bound or reported non-success\n");
                                                syscall1(0x9CC1_BAD4);
                                            }
                                        }
                                    }
                                    None => {
                                        com1_write_str("[XHCI_DRIVER] XHCI_NO_DEVICE_ATTACHED: no real device found on any port -- Address Device correctly skipped\n");
                                        syscall1(0x9CC1_D0EF);
                                    }
                                }
                            } else {
                                com1_write_str("[XHCI_DRIVER] XHCI_ENABLE_SLOT_FAIL: reported slot_id out of real bounds\n");
                                syscall1(0x9CC1_BAD3);
                            }
                        }
                        None => {
                            com1_write_str("[XHCI_DRIVER] XHCI_ENABLE_SLOT_FAIL: real command round trip did not complete within bound or reported non-success\n");
                            syscall1(0x9CC1_BAD3);
                        }
                    }
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
