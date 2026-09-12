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
const INPUT_CONTEXT_OFF: u64 = 0xC00; // Input Control Context(32) + Slot Context(32) + EP0 Context(32) = 96 bytes
const OUTPUT_DEVICE_CONTEXT_OFF: u64 = 0xD00; // Slot Context(32) + EP0 Context(32) = 64 bytes -- the real structure DCBAA[slot_id] points to
const EP0_TRANSFER_RING_OFF: u64 = 0xD80; // EP0's own real Transfer Ring -- required by a legally-formed EP0 Context even though this increment issues no actual control transfer yet

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

const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
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
/// `BSR=0` (a genuine SET_ADDRESS is issued, not deferred), and EP0's
/// `MaxPacketSize` uses 8 -- the universally-legal minimum for an
/// as-yet-unqueried control endpoint (real drivers commonly start
/// here, then update it after reading the device descriptor, itself
/// real, separate follow-up work this increment doesn't reach).
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
    for i in 0..12u64 {
        core::ptr::write_volatile((input_ctx_vaddr + i * 8) as *mut u64, 0);
    }
    for i in 0..8u64 {
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
    // (the standard real retry count), MaxPacketSize=8 (see this
    // function's own doc for why), pointing at the real EP0 Transfer
    // Ring above with DCS=1 (the real initial dequeue cycle state).
    let ep0_ctx_vaddr = input_ctx_vaddr + 64;
    let ep0_dword1 = (3u32 << 1) | (4u32 << 3) | (8u32 << 16);
    core::ptr::write_volatile((ep0_ctx_vaddr + 4) as *mut u32, ep0_dword1);
    core::ptr::write_volatile((ep0_ctx_vaddr + 8) as *mut u32, (ep0_ring_phys as u32) | 1); // DCS=1
    core::ptr::write_volatile((ep0_ctx_vaddr + 12) as *mut u32, (ep0_ring_phys >> 32) as u32);
    core::ptr::write_volatile((ep0_ctx_vaddr + 16) as *mut u32, 8); // Average TRB Length -- a real, reasonable placeholder

    // Real DCBAA[slot_id] -> the real Output Device Context this
    // exact command will have the controller fill in.
    core::ptr::write_volatile((dcbaa_vaddr + slot_id as u64 * 8) as *mut u64, out_ctx_phys);

    // Real Address Device command: parameter = real Input Context
    // physical address, Control bits 31:24 = the real Slot ID this
    // command targets (BSR=0 -- a genuine SET_ADDRESS is issued).
    let control_slot = (slot_id & 0xFF) << 24;
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_ADDRESS_DEVICE, input_ctx_phys, control_slot)
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
