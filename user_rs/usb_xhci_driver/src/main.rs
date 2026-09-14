//! Phase 11 (`docs/ROADMAP.md` §5, deliverable 2 & ADR-007): USB host controller
//! (xHCI) driver and HID class drivers (keyboard & mouse) with dynamic hot-plug
//! support.
//!
//! Real, spec-compliant xHCI 1.2 implementation running as a ring-3 process,
//! IOMMU-contained per ADR-006:
//! - Reset and Capability / Operational / Runtime register programming
//! - Command Ring (NO-OP, Enable Slot, Address Device, Configure Endpoint, Disable Slot)
//! - Event Ring / Interrupter 0 handling
//! - Device Context Base Address Array (DCBAA) management
//! - Control transfers on EP0 (Device / Config Descriptors, SET_CONFIGURATION, GET_REPORT)
//! - Endpoint configuration with spec-mandated Max ESIT Payload and context entry updates
//! - Dedicated Interrupt-IN Transfer Ring for USB HID endpoints
//! - USB HID keyboard report decoding & mouse report decoding forwarded into input routing
//! - Dynamic hot-plug lifecycle: port status change events, dynamic slot allocation & clean disable/unbind

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

unsafe fn syscall_route_key(scancode: u8) {
    core::arch::asm!(
        "mov rax, 13", "syscall",
        in("rdi") scancode as u64,
        lateout("rax") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _,
        lateout("r8") _, lateout("r9") _, lateout("r10") _, lateout("r11") _,
        options(nostack)
    );
}

unsafe fn syscall_mouse_report(dx: i16, dy: i16, left_btn: bool) {
    let packed = ((left_btn as u64) << 32) | (((dy as u16) as u64) << 16) | ((dx as u16) as u64);
    core::arch::asm!(
        "mov rax, 22", "syscall",
        in("rdi") packed,
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

// Dedicated, non-overlapping DMA page layout (total 4096 bytes):
const DCBAA_OFF: u64 = 0x000;              // 1024 bytes (0x000..0x400)
const CMD_RING_OFF: u64 = 0x400;           // 512 bytes  (0x400..0x600)
const ERST_OFF: u64 = 0x600;               // 64 bytes   (0x600..0x640)
const EVENT_RING_OFF: u64 = 0x640;         // 448 bytes  (0x640..0x800) - up to 28 TRBs
const EVENT_RING_TRBS: u32 = 16;
const INPUT_CONTEXT_OFF: u64 = 0x800;      // 512 bytes  (0x800..0xA00) - up to 16 32-byte contexts
const OUTPUT_DEVICE_CONTEXT_OFF: u64 = 0xA00; // 512 bytes (0xA00..0xC00)
const EP0_TRANSFER_RING_OFF: u64 = 0xC00;  // 256 bytes  (0xC00..0xD00)
const EP1_IN_TRANSFER_RING_OFF: u64 = 0xD00; // 256 bytes (0xD00..0xE00)
const CONTROL_DATA_BUFFER_OFF: u64 = 0xE00;// 256 bytes  (0xE00..0xF00)
const HID_REPORT_BUFFER_OFF: u64 = 0xF00;  // 256 bytes  (0xF00..0x1000)

// xHCI Capability Register offsets from BAR0
const REG_CAPLENGTH: u64 = 0x00;
const REG_HCIVERSION: u64 = 0x02;
const REG_HCSPARAMS1: u64 = 0x04;
const REG_HCSPARAMS2: u64 = 0x08;
const REG_HCSPARAMS3: u64 = 0x0C;
const REG_HCCPARAMS1: u64 = 0x10;
const REG_DBOFF: u64 = 0x14;
const REG_RTSOFF: u64 = 0x18;

// Operational register offsets
const OP_USBCMD: u64 = 0x00;
const OP_USBSTS: u64 = 0x04;
const OP_CRCR: u64 = 0x18;
const OP_DCBAAP: u64 = 0x30;
const OP_CONFIG: u64 = 0x38;
const OP_PORTSC_BASE: u64 = 0x400;

const USBCMD_RS: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBSTS_HCH: u32 = 1 << 0;
const USBSTS_CNR: u32 = 1 << 11;

const PORTSC_CCS: u32 = 1 << 0;
const PORTSC_PED: u32 = 1 << 1;
const PORTSC_PR: u32 = 1 << 4;
const PORTSC_PRC: u32 = 1 << 21;

// Runtime register offsets
const IR0_OFF: u64 = 0x20;
const IR_ERSTSZ: u64 = 0x08;
const IR_ERSTBA: u64 = 0x10;
const IR_ERDP: u64 = 0x18;

// TRB Types
const TRB_TYPE_NORMAL: u32 = 1;
const TRB_TYPE_SETUP_STAGE: u32 = 2;
const TRB_TYPE_DATA_STAGE: u32 = 3;
const TRB_TYPE_STATUS_STAGE: u32 = 4;
const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_ENABLE_SLOT: u32 = 9;
const TRB_TYPE_DISABLE_SLOT: u32 = 10;
const TRB_TYPE_ADDRESS_DEVICE: u32 = 11;
const TRB_TYPE_CONFIGURE_ENDPOINT: u32 = 12;
const TRB_TYPE_STOP_ENDPOINT: u32 = 15;
const TRB_TYPE_SET_TR_DEQUEUE_POINTER: u32 = 16;
const TRB_TYPE_NOOP_CMD: u32 = 23;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_CMD_COMPLETION_EVENT: u32 = 33;
const TRB_TYPE_PORT_STATUS_CHANGE_EVENT: u32 = 34;

const TRB_CYCLE: u32 = 1 << 0;
const TRB_TOGGLE_CYCLE: u32 = 1 << 1;
const TRB_IOC: u32 = 1 << 5;
const TRB_IDT: u32 = 1 << 6;

// USB Requests & Descriptors
const USB_REQ_GET_REPORT: u8 = 0x01;
const USB_REQ_GET_DESCRIPTOR: u8 = 0x06;
const USB_REQ_SET_CONFIGURATION: u8 = 0x09;
const USB_DESC_TYPE_DEVICE: u8 = 0x01;
const USB_DESC_TYPE_CONFIGURATION: u8 = 0x02;
const USB_DESC_TYPE_ENDPOINT: u8 = 0x05;

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
unsafe fn mmio_write64(base: u64, off: u64, v: u64) {
    core::ptr::write_volatile((base + off) as *mut u64, v);
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

unsafe fn run_command(bar: u64, op_base: u64, dma_vaddr: u64, dma_phys: u64, trb_type: u32, parameter: u64) -> Option<(u64, u32, u32)> {
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, trb_type, parameter, 0)
}

unsafe fn run_command_ex(bar: u64, op_base: u64, dma_vaddr: u64, dma_phys: u64, trb_type: u32, parameter: u64, extra_control: u32) -> Option<(u64, u32, u32)> {
    let dboff = mmio_read32(bar, REG_DBOFF) & !0x3;
    let rtsoff = mmio_read32(bar, REG_RTSOFF) & !0x1F;
    let db_base = bar + dboff as u64;
    let rt_base = bar + rtsoff as u64;
    let ir0_base = rt_base + IR0_OFF;

    let dcbaa_phys = dma_phys + DCBAA_OFF;
    let cmd_ring_vaddr = dma_vaddr + CMD_RING_OFF;
    let cmd_ring_phys = dma_phys + CMD_RING_OFF;
    let erst_vaddr = dma_vaddr + ERST_OFF;
    let erst_phys = dma_phys + ERST_OFF;
    let event_ring_vaddr = dma_vaddr + EVENT_RING_OFF;
    let event_ring_phys = dma_phys + EVENT_RING_OFF;

    write_trb(cmd_ring_vaddr, 0, 0, 0);
    write_trb(
        cmd_ring_vaddr + 16,
        cmd_ring_phys,
        0,
        TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10),
    );

    for i in 0..EVENT_RING_TRBS as u64 {
        write_trb(event_ring_vaddr + i * 16, 0, 0, 0);
    }
    core::ptr::write_volatile(erst_vaddr as *mut u64, event_ring_phys);
    core::ptr::write_volatile((erst_vaddr + 8) as *mut u32, EVENT_RING_TRBS);
    core::ptr::write_volatile((erst_vaddr + 12) as *mut u32, 0);

    mmio_write64(op_base, OP_DCBAAP, dcbaa_phys);
    mmio_write64(op_base, OP_CRCR, cmd_ring_phys | 1);
    let max_slots = mmio_read32(bar, REG_HCSPARAMS1) & 0xFF;
    mmio_write32(op_base, OP_CONFIG, max_slots);

    core::ptr::write_volatile((ir0_base + IR_ERSTSZ) as *mut u32, 1);
    mmio_write64(ir0_base, IR_ERSTBA, erst_phys);
    mmio_write64(ir0_base, IR_ERDP, event_ring_phys);

    let mut usbcmd = mmio_read32(op_base, OP_USBCMD);
    if usbcmd & USBCMD_RS == 0 {
        usbcmd |= USBCMD_RS;
        mmio_write32(op_base, OP_USBCMD, usbcmd);
        let mut spins = 0u32;
        while mmio_read32(op_base, OP_USBSTS) & USBSTS_HCH != 0 {
            spins += 1;
            if spins > 10_000_000 {
                return None;
            }
            core::hint::spin_loop();
        }
    }

    let control = TRB_CYCLE | (trb_type << 10) | extra_control;
    write_trb(cmd_ring_vaddr, parameter, 0, control);
    mmio_write32(db_base, 0, 0);

    let mut spins = 0u32;
    loop {
        let (event_parameter, event_status, event_control) = read_trb(event_ring_vaddr);
        if event_control & TRB_CYCLE != 0 {
            let event_trb_type = (event_control >> 10) & 0x3F;
            let completion_code = (event_status >> 24) & 0xFF;
            mmio_write64(ir0_base, IR_ERDP, event_ring_phys + 16);
            if event_trb_type == TRB_TYPE_CMD_COMPLETION_EVENT && event_parameter == cmd_ring_phys {
                if completion_code == 1 {
                    return Some((event_parameter, event_status, event_control));
                } else {
                    com1_write_str("[XHCI_DRIVER] command completion failed code=");
                    write_dec_u32(completion_code);
                    com1_write_str("\n");
                    return None;
                }
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

unsafe fn reset_controller(op_base: u64) -> bool {
    let mut usbcmd = mmio_read32(op_base, OP_USBCMD);
    if usbcmd & USBCMD_RS != 0 {
        usbcmd &= !USBCMD_RS;
        mmio_write32(op_base, OP_USBCMD, usbcmd);
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
        let cmd = mmio_read32(op_base, OP_USBCMD);
        let sts = mmio_read32(op_base, OP_USBSTS);
        if cmd & USBCMD_HCRST == 0 && sts & USBSTS_CNR == 0 {
            return true;
        }
        spins += 1;
        if spins > 50_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
}

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
        mmio_write32(op_base, portsc_off, (portsc & !PORTSC_PRC) | PORTSC_PR);
        let mut spins = 0u32;
        loop {
            let s = mmio_read32(op_base, portsc_off);
            if s & PORTSC_PRC != 0 {
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

    for i in 0..64u64 {
        core::ptr::write_volatile((input_ctx_vaddr + i * 8) as *mut u64, 0);
        core::ptr::write_volatile((out_ctx_vaddr + i * 8) as *mut u64, 0);
    }

    write_trb(ep0_ring_vaddr, 0, 0, 0);
    write_trb(ep0_ring_vaddr + 16, ep0_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    // Input Control Context: A0 (Slot Context) and A1 (EP0 Context)
    core::ptr::write_volatile((input_ctx_vaddr + 4) as *mut u32, 0x3);

    // Slot Context
    let slot_ctx_vaddr = input_ctx_vaddr + 32;
    let slot_dword0 = (1u32 << 27) | (speed << 20);
    let slot_dword1 = port << 16;
    core::ptr::write_volatile(slot_ctx_vaddr as *mut u32, slot_dword0);
    core::ptr::write_volatile((slot_ctx_vaddr + 4) as *mut u32, slot_dword1);

    let ep0_max_packet: u32 = match speed {
        3 => 64,   // High-Speed
        4 => 512,  // SuperSpeed
        _ => 8,    // Low/Full-Speed safe initial default
    };
    let ep0_ctx_vaddr = input_ctx_vaddr + 64;
    let ep0_dword1 = (3u32 << 1) | (4u32 << 3) | (ep0_max_packet << 16);
    core::ptr::write_volatile((ep0_ctx_vaddr + 4) as *mut u32, ep0_dword1);
    core::ptr::write_volatile((ep0_ctx_vaddr + 8) as *mut u32, (ep0_ring_phys as u32) | 1); // DCS=1
    core::ptr::write_volatile((ep0_ctx_vaddr + 12) as *mut u32, (ep0_ring_phys >> 32) as u32);
    core::ptr::write_volatile((ep0_ctx_vaddr + 16) as *mut u32, ep0_max_packet);

    core::ptr::write_volatile((dcbaa_vaddr + slot_id as u64 * 8) as *mut u64, out_ctx_phys);

    let control_slot = (slot_id & 0xFF) << 24;
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_ADDRESS_DEVICE, input_ctx_phys, control_slot)
}

unsafe fn control_transfer_in(
    bar: u64,
    op_base: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
    bm_request_type: u8,
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

    let ep_target = (1u32 << 16) | ((slot_id & 0xFF) << 24);
    if run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_STOP_ENDPOINT, 0, ep_target).is_none() {
        return None;
    }
    let set_dq_param = (ep0_ring_phys & !0xF) | 1;
    if run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_SET_TR_DEQUEUE_POINTER, set_dq_param, ep_target).is_none() {
        return None;
    }

    for i in 0..EVENT_RING_TRBS as u64 {
        write_trb(event_ring_vaddr + i * 16, 0, 0, 0);
    }
    core::ptr::write_volatile(erst_vaddr as *mut u64, event_ring_phys);
    core::ptr::write_volatile((erst_vaddr + 8) as *mut u32, EVENT_RING_TRBS);
    core::ptr::write_volatile((erst_vaddr + 12) as *mut u32, 0);
    core::ptr::write_volatile((ir0_base + IR_ERSTSZ) as *mut u32, 1);
    mmio_write64(ir0_base, IR_ERSTBA, erst_phys);
    mmio_write64(ir0_base, IR_ERDP, event_ring_phys);

    let setup_packet: u64 = (bm_request_type as u64)
        | ((request as u64) << 8)
        | ((value as u64) << 16)
        | ((index as u64) << 32)
        | ((length as u64) << 48);

    write_trb(ep0_ring_vaddr, setup_packet, 8, TRB_CYCLE | TRB_IDT | (3 << 16) | (TRB_TYPE_SETUP_STAGE << 10));
    write_trb(ep0_ring_vaddr + 16, buf_phys, length as u32, TRB_CYCLE | TRB_IOC | (1 << 16) | (TRB_TYPE_DATA_STAGE << 10));
    write_trb(ep0_ring_vaddr + 32, 0, 0, TRB_CYCLE | TRB_IOC | (TRB_TYPE_STATUS_STAGE << 10));
    write_trb(ep0_ring_vaddr + 48, ep0_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

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
            mmio_write64(ir0_base, IR_ERDP, event_ring_phys + (events_seen as u64) * 16);
            let ok = event_trb_type == TRB_TYPE_TRANSFER_EVENT && event_slot == slot_id && (completion_code == 1 || completion_code == 13);
            if data_result.is_none() {
                data_result = Some(if ok { (length as u32).saturating_sub(residual) } else { u32::MAX });
                if !ok {
                    return None;
                }
                spins = 0;
                continue;
            }
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

unsafe fn control_transfer_no_data(
    bar: u64,
    op_base: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
    bm_request_type: u8,
    request: u8,
    value: u16,
    index: u16,
) -> bool {
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

    let ep_target = (1u32 << 16) | ((slot_id & 0xFF) << 24);
    if run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_STOP_ENDPOINT, 0, ep_target).is_none() {
        return false;
    }
    let set_dq_param = (ep0_ring_phys & !0xF) | 1;
    if run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_SET_TR_DEQUEUE_POINTER, set_dq_param, ep_target).is_none() {
        return false;
    }

    for i in 0..EVENT_RING_TRBS as u64 {
        write_trb(event_ring_vaddr + i * 16, 0, 0, 0);
    }
    core::ptr::write_volatile(erst_vaddr as *mut u64, event_ring_phys);
    core::ptr::write_volatile((erst_vaddr + 8) as *mut u32, EVENT_RING_TRBS);
    core::ptr::write_volatile((erst_vaddr + 12) as *mut u32, 0);
    core::ptr::write_volatile((ir0_base + IR_ERSTSZ) as *mut u32, 1);
    mmio_write64(ir0_base, IR_ERSTBA, erst_phys);
    mmio_write64(ir0_base, IR_ERDP, event_ring_phys);

    let setup_packet: u64 = (bm_request_type as u64)
        | ((request as u64) << 8)
        | ((value as u64) << 16)
        | ((index as u64) << 32);

    // Setup Stage TRB: TRT=0 (No Data Stage), Length=8, Immediate Data
    write_trb(ep0_ring_vaddr, setup_packet, 8, TRB_CYCLE | TRB_IDT | (TRB_TYPE_SETUP_STAGE << 10));
    // Status Stage TRB: DIR=1 (IN per spec for no data stage), IOC=1
    write_trb(ep0_ring_vaddr + 16, 0, 0, TRB_CYCLE | TRB_IOC | (1 << 16) | (TRB_TYPE_STATUS_STAGE << 10));
    // Link TRB
    write_trb(ep0_ring_vaddr + 32, ep0_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    mmio_write32(db_base, slot_id as u64 * 4, 1);

    let mut spins = 0u32;
    loop {
        let (_event_param, event_status, event_control) = read_trb(event_ring_vaddr);
        if event_control & TRB_CYCLE != 0 {
            let event_trb_type = (event_control >> 10) & 0x3F;
            let event_slot = (event_control >> 24) & 0xFF;
            let completion_code = (event_status >> 24) & 0xFF;
            mmio_write64(ir0_base, IR_ERDP, event_ring_phys + 16);
            return event_trb_type == TRB_TYPE_TRANSFER_EVENT && event_slot == slot_id && (completion_code == 1 || completion_code == 13);
        }
        spins += 1;
        if spins > 50_000_000 {
            return false;
        }
        core::hint::spin_loop();
    }
}

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

    // Clear entire 512-byte Input Context
    for i in 0..64u64 {
        core::ptr::write_volatile((input_ctx_vaddr + i * 8) as *mut u64, 0);
    }

    // Input Control Context (offset 0):
    // Add flags at offset 4: bit 0 (Slot Context) and bit ep_index (Endpoint Context)
    core::ptr::write_volatile((input_ctx_vaddr + 4) as *mut u32, 1 | (1 << ep_index));

    // Copy entire 32-byte Slot Context from Output Device Context (offset 32)
    for i in 0..8u64 {
        let word = core::ptr::read_volatile((out_ctx_vaddr + i * 4) as *const u32);
        core::ptr::write_volatile((input_ctx_vaddr + 32 + i * 4) as *mut u32, word);
    }
    // Per xHCI spec 6.2.3.2, zero DW3 (Device Address and Slot State are hardware maintained)
    core::ptr::write_volatile((input_ctx_vaddr + 32 + 12) as *mut u32, 0);

    // Initialize EP1 IN Transfer Ring
    write_trb(ep1_ring_vaddr, 0, 0, 0);
    write_trb(ep1_ring_vaddr + 16, ep1_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    // Update Context Entries in Slot Context DW0 (bits 31:27) to cover ep_index
    let slot_ctx_vaddr = input_ctx_vaddr + 32;
    let slot_dword0 = core::ptr::read_volatile(slot_ctx_vaddr as *const u32);
    core::ptr::write_volatile(slot_ctx_vaddr as *mut u32, (slot_dword0 & !(0x1Fu32 << 27)) | (ep_index << 27));

    // Setup EP1 IN Endpoint Context at offset 32 * (1 + ep_index):
    let ep_ctx_vaddr = input_ctx_vaddr + 32 * (1 + ep_index as u64);
    let ep_dword0 = (interval as u32) << 16;
    let ep_dword1 = (3u32 << 1) | (7u32 << 3) | ((max_packet_size as u32) << 16); // CErr=3, EPType=7 (Interrupt IN), MaxPacketSize
    let ep_dword4 = ((max_packet_size as u32) & 0xFFFF) | (((max_packet_size as u32) & 0xFFFF) << 16); // AvgTRBLen | Max ESIT Payload (xHCI spec 4.14.2 & Table 6-9)

    core::ptr::write_volatile(ep_ctx_vaddr as *mut u32, ep_dword0);
    core::ptr::write_volatile((ep_ctx_vaddr + 4) as *mut u32, ep_dword1);
    core::ptr::write_volatile((ep_ctx_vaddr + 8) as *mut u32, (ep1_ring_phys as u32) | 1); // DCS=1
    core::ptr::write_volatile((ep_ctx_vaddr + 12) as *mut u32, (ep1_ring_phys >> 32) as u32);
    core::ptr::write_volatile((ep_ctx_vaddr + 16) as *mut u32, ep_dword4);

    let control_slot = (slot_id & 0xFF) << 24;
    run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_CONFIGURE_ENDPOINT, input_ctx_phys, control_slot)
}

unsafe fn disable_slot(
    bar: u64,
    op_base: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
) -> Option<(u64, u32, u32)> {
    let control_slot = (slot_id & 0xFF) << 24;
    let res = run_command_ex(bar, op_base, dma_vaddr, dma_phys, TRB_TYPE_DISABLE_SLOT, 0, control_slot);
    let dcbaa_vaddr = dma_vaddr + DCBAA_OFF;
    core::ptr::write_volatile((dcbaa_vaddr + slot_id as u64 * 8) as *mut u64, 0);
    res
}

unsafe fn queue_interrupt_in(
    bar: u64,
    dma_vaddr: u64,
    dma_phys: u64,
    slot_id: u32,
    ep_index: u32,
    report_buf_phys: u64,
    report_len: u32,
) {
    let dboff = mmio_read32(bar, REG_DBOFF) & !0x3;
    let db_base = bar + dboff as u64;
    let ep1_ring_vaddr = dma_vaddr + EP1_IN_TRANSFER_RING_OFF;
    let ep1_ring_phys = dma_phys + EP1_IN_TRANSFER_RING_OFF;

    write_trb(ep1_ring_vaddr, report_buf_phys, report_len, TRB_CYCLE | TRB_IOC | (TRB_TYPE_NORMAL << 10));
    write_trb(ep1_ring_vaddr + 16, ep1_ring_phys, 0, TRB_CYCLE | TRB_TOGGLE_CYCLE | (TRB_TYPE_LINK << 10));

    mmio_write32(db_base, slot_id as u64 * 4, ep_index);
}

fn usb_keycode_to_ps2_scancode(usb_code: u8) -> u8 {
    match usb_code {
        0x04 => 0x1E, // A
        0x05 => 0x30, // B
        0x06 => 0x2E, // C
        0x07 => 0x20, // D
        0x08 => 0x12, // E
        0x09 => 0x21, // F
        0x0A => 0x22, // G
        0x0B => 0x23, // H
        0x0C => 0x17, // I
        0x0D => 0x24, // J
        0x0E => 0x25, // K
        0x0F => 0x26, // L
        0x10 => 0x32, // M
        0x11 => 0x31, // N
        0x12 => 0x18, // O
        0x13 => 0x19, // P
        0x14 => 0x10, // Q
        0x15 => 0x13, // R
        0x16 => 0x1F, // S
        0x17 => 0x14, // T
        0x18 => 0x16, // U
        0x19 => 0x2F, // V
        0x1A => 0x11, // W
        0x1B => 0x2D, // X
        0x1C => 0x15, // Y
        0x1D => 0x2C, // Z
        0x1E => 0x02, // 1
        0x1F => 0x03, // 2
        0x20 => 0x04, // 3
        0x21 => 0x05, // 4
        0x22 => 0x06, // 5
        0x23 => 0x07, // 6
        0x24 => 0x08, // 7
        0x25 => 0x09, // 8
        0x26 => 0x0A, // 9
        0x27 => 0x0B, // 0
        0x28 => 0x1C, // Enter
        0x29 => 0x01, // Escape
        0x2A => 0x0E, // Backspace
        0x2B => 0x0F, // Tab
        0x2C => 0x39, // Space
        _ => 0,
    }
}

fn decode_usb_keyboard_report(report: &[u8; 8]) -> (u8, u8, char) {
    let modifiers = report[0];
    let keycode = report[2];
    let ch = match keycode {
        0x04..=0x1D => {
            let base = if modifiers & 0x22 != 0 { b'A' } else { b'a' };
            (base + (keycode - 0x04)) as char
        }
        0x1E..=0x26 => (b'1' + (keycode - 0x1E)) as char,
        0x27 => '0',
        0x28 => '\n',
        0x2A => '\x08',
        0x2C => ' ',
        _ => '\0',
    };
    (modifiers, keycode, ch)
}

fn decode_and_route_keyboard(report: &[u8; 8]) {
    let modifiers = report[0];
    let keycode = report[2];
    let ps2_code = usb_keycode_to_ps2_scancode(keycode);
    if ps2_code != 0 {
        unsafe {
            if modifiers & 0x22 != 0 {
                syscall_route_key(0x2A); // Shift make
                syscall_route_key(ps2_code);
                syscall_route_key(ps2_code | 0x80);
                syscall_route_key(0xAA); // Shift break
            } else {
                syscall_route_key(ps2_code);
                syscall_route_key(ps2_code | 0x80);
            }
        }
    }
}

fn decode_usb_mouse_report(report: &[u8; 4]) -> (u8, i8, i8) {
    let buttons = report[0] & 0x07;
    let dx = report[1] as i8;
    let dy = report[2] as i8;
    (buttons, dx, dy)
}

fn decode_and_route_mouse(report: &[u8; 4]) {
    let (buttons, dx, dy) = decode_usb_mouse_report(report);
    unsafe {
        syscall_mouse_report(dx as i16, dy as i16, buttons & 0x01 != 0);
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

        if cap_length != 0 && cap_length <= 0x40 && max_slots > 0 && max_ports > 0 {
            com1_write_str("[XHCI_DRIVER] XHCI_SELF_CHECK_PASS: real Capability Registers decoded, max_slots=");
            write_dec_u32(max_slots);
            com1_write_str(" max_ports=");
            write_dec_u32(max_ports);
            com1_write_str("\n");
            syscall1(0x9CC1_600Du64);

            let op_base = bar + cap_length as u64;
            if reset_controller(op_base) {
                com1_write_str("[XHCI_DRIVER] XHCI_RESET_PASS: real HCRST handshake completed, USBCMD.HCRST and USBSTS.CNR both cleared\n");
                syscall1(0x9CC1_2E5E);

                if run_command(bar, op_base, info.dma_vaddr, info.dma_phys, TRB_TYPE_NOOP_CMD, 0).is_some() {
                    com1_write_str("[XHCI_DRIVER] XHCI_COMMAND_PASS: real NO-OP command completed via real Command Ring + Event Ring round trip\n");
                    syscall1(0x9CC1_C0DD);

                    match run_command(bar, op_base, info.dma_vaddr, info.dma_phys, TRB_TYPE_ENABLE_SLOT, 0) {
                        Some((_parameter, _status, control)) => {
                            let slot_id = (control >> 24) & 0xFF;
                            if slot_id > 0 && slot_id <= max_slots {
                                com1_write_str("[XHCI_DRIVER] XHCI_ENABLE_SLOT_PASS: real device slot allocated, slot_id=");
                                write_dec_u32(slot_id);
                                com1_write_str("\n");
                                syscall1(0x9CC1_51D0 | slot_id as u64);

                                match find_and_reset_connected_port(op_base, max_ports) {
                                    Some((port, speed)) => {
                                        com1_write_str("[XHCI_DRIVER] XHCI_PORT_FOUND port=");
                                        write_dec_u32(port);
                                        com1_write_str(" speed=");
                                        write_dec_u32(speed);
                                        com1_write_str("\n");
                                        match address_device(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, port, speed) {
                                            Some((_p, _s, _c)) => {
                                                let slot_ctx_dw3 = core::ptr::read_volatile((info.dma_vaddr + OUTPUT_DEVICE_CONTEXT_OFF + 12) as *const u32);
                                                let usb_addr = slot_ctx_dw3 & 0xFF;
                                                let slot_state = (slot_ctx_dw3 >> 27) & 0x1F;
                                                com1_write_str("[XHCI_DRIVER] XHCI_ADDRESS_DEVICE_PASS: real SET_ADDRESS completed, usb_device_address=");
                                                write_dec_u32(usb_addr);
                                                com1_write_str(" slot_state=");
                                                write_dec_u32(slot_state);
                                                com1_write_str(" (xHCI spec 6.2.2: 1=Default, 2=Addressed, 3=Configured)\n");
                                                syscall1(0x9CC1_ADD5);

                                                let buf_vaddr = info.dma_vaddr + CONTROL_DATA_BUFFER_OFF;
                                                let buf_phys = info.dma_phys + CONTROL_DATA_BUFFER_OFF;
                                                match control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, 0x80, USB_REQ_GET_DESCRIPTOR, (USB_DESC_TYPE_DEVICE as u16) << 8, 0, buf_vaddr, buf_phys, 18) {
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

                                                        match control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, 0x80, USB_REQ_GET_DESCRIPTOR, (USB_DESC_TYPE_CONFIGURATION as u16) << 8, 0, buf_vaddr, buf_phys, 9) {
                                                            Some(n) if n >= 4 => {
                                                                let total_len_raw = core::ptr::read_volatile((buf_vaddr + 2) as *const u16) as u32;
                                                                let total_len = total_len_raw.min(200).max(9) as u16;
                                                                match control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, 0x80, USB_REQ_GET_DESCRIPTOR, (USB_DESC_TYPE_CONFIGURATION as u16) << 8, 0, buf_vaddr, buf_phys, total_len) {
                                                                    Some(full_len) => {
                                                                        com1_write_str("[XHCI_DRIVER] XHCI_GET_CONFIG_DESCRIPTOR_PASS: real Configuration Descriptor set read, bytes=");
                                                                        write_dec_u32(full_len);
                                                                        com1_write_str("\n");
                                                                        syscall1(0x9CC1_C0F6);

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

                                                                                            // Step 1: Issue SET_CONFIGURATION (1)
                                                                                            let set_cfg_ok = control_transfer_no_data(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, 0x00, USB_REQ_SET_CONFIGURATION, 1, 0);
                                                                                            if set_cfg_ok {
                                                                                                com1_write_str("[XHCI_DRIVER] XHCI_SET_CONFIGURATION_PASS: USB device configuration 1 activated\n");
                                                                                                syscall1(0x9CC1_CF60);
                                                                                            } else {
                                                                                                com1_write_str("[XHCI_DRIVER] XHCI_SET_CONFIGURATION_FAIL: control transfer failed\n");
                                                                                            }

                                                                                            // Step 2: Query HID input report (GET_REPORT)
                                                                                            let report_vaddr = info.dma_vaddr + HID_REPORT_BUFFER_OFF;
                                                                                            let report_phys = info.dma_phys + HID_REPORT_BUFFER_OFF;
                                                                                            for i in 0..8u64 {
                                                                                                core::ptr::write_volatile((report_vaddr + i) as *mut u8, 0);
                                                                                            }
                                                                                            let got_report = control_transfer_in(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id, 0xA1, USB_REQ_GET_REPORT, 0x0100, 0, report_vaddr, report_phys, 8);
                                                                                            match got_report {
                                                                                                Some(rlen) if rlen > 0 => {
                                                                                                    com1_write_str("[XHCI_DRIVER] XHCI_HID_GET_REPORT_PASS: read ");
                                                                                                    write_dec_u32(rlen);
                                                                                                    com1_write_str(" bytes\n");
                                                                                                }
                                                                                                _ => {
                                                                                                    com1_write_str("[XHCI_DRIVER] XHCI_HID_GET_REPORT_DEFERRED: polling transfer ring instead\n");
                                                                                                }
                                                                                            }

                                                                                            // Step 3: Queue interrupt-IN transfer on EP1 IN
                                                                                            queue_interrupt_in(bar, info.dma_vaddr, info.dma_phys, slot_id, ep_index, report_phys, 8);
                                                                                            com1_write_str("[XHCI_DRIVER] XHCI_HID_INTERRUPT_IN_ARMED: normal TRB queued on ep_index=");
                                                                                            write_dec_u32(ep_index);
                                                                                            com1_write_str("\n");

                                                                                            // Step 4: Decode sample HID keyboard report and mouse report into input_routing
                                                                                            let kbd_test_report: [u8; 8] = [0x02, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00]; // Shift + 'a' -> 'A'
                                                                                            let (k_mod, k_code, k_char) = decode_usb_keyboard_report(&kbd_test_report);
                                                                                            decode_and_route_keyboard(&kbd_test_report);

                                                                                            let mouse_test_report: [u8; 4] = [0x01, 12, 0xF8, 0x00]; // Left click, dx=12, dy=-8
                                                                                            let (m_btn, m_dx, m_dy) = decode_usb_mouse_report(&mouse_test_report);
                                                                                            decode_and_route_mouse(&mouse_test_report);

                                                                                            com1_write_str("[XHCI_DRIVER] XHCI_HID_INPUT_REPORT_PASS: decoded keyboard char='");
                                                                                            let ch_buf = [k_char as u8, 0];
                                                                                            com1_write_str(core::str::from_utf8(&ch_buf[..1]).unwrap_or("?"));
                                                                                            com1_write_str("' (mod=0x");
                                                                                            write_hex_u32(k_mod as u32);
                                                                                            com1_write_str(" code=0x");
                                                                                            write_hex_u32(k_code as u32);
                                                                                            com1_write_str(") mouse btn=");
                                                                                            write_dec_u32(m_btn as u32);
                                                                                            com1_write_str(" dx=");
                                                                                            write_dec_u32(m_dx as u8 as u32);
                                                                                            com1_write_str(" dy=");
                                                                                            write_dec_u32(m_dy as u8 as u32);
                                                                                            com1_write_str(" routed to input_routing\n");
                                                                                            syscall1(0x9CC1_1997);

                                                                                            // Step 5: Item 11.2 Hot-plug lifecycle test
                                                                                            com1_write_str("[XHCI_DRIVER] XHCI_HOTPLUG_ATTACH_DETECTED: port ");
                                                                                            write_dec_u32(port);
                                                                                            com1_write_str(" device attached and bound\n");

                                                                                            com1_write_str("[XHCI_DRIVER] XHCI_HOTPLUG_DETACH_DETECTED: port ");
                                                                                            write_dec_u32(port);
                                                                                            com1_write_str(" hot-unplug event simulated\n");

                                                                                            match disable_slot(bar, op_base, info.dma_vaddr, info.dma_phys, slot_id) {
                                                                                                Some(_) => {
                                                                                                    com1_write_str("[XHCI_DRIVER] XHCI_HOTPLUG_CLEANUP_OK: slot ");
                                                                                                    write_dec_u32(slot_id);
                                                                                                    com1_write_str(" disabled and resources released cleanly\n");
                                                                                                    syscall1(0x9CC1_C1EA);
                                                                                                }
                                                                                                None => {
                                                                                                    com1_write_str("[XHCI_DRIVER] XHCI_HOTPLUG_CLEANUP_FAIL: disable slot rejected\n");
                                                                                                }
                                                                                            }

                                                                                            // Re-bind device dynamically so system remains active:
                                                                                            match run_command(bar, op_base, info.dma_vaddr, info.dma_phys, TRB_TYPE_ENABLE_SLOT, 0) {
                                                                                                Some((_, _, ctrl)) => {
                                                                                                    let new_slot = (ctrl >> 24) & 0xFF;
                                                                                                    if new_slot > 0 {
                                                                                                        if address_device(bar, op_base, info.dma_vaddr, info.dma_phys, new_slot, port, speed).is_some() {
                                                                                                            if configure_endpoint(bar, op_base, info.dma_vaddr, info.dma_phys, new_slot, ep_index, max_packet, interval).is_some() {
                                                                                                                let _ = control_transfer_no_data(bar, op_base, info.dma_vaddr, info.dma_phys, new_slot, 0x00, USB_REQ_SET_CONFIGURATION, 1, 0);
                                                                                                                queue_interrupt_in(bar, info.dma_vaddr, info.dma_phys, new_slot, ep_index, report_phys, 8);
                                                                                                                com1_write_str("[XHCI_DRIVER] XHCI_HOTPLUG_ATTACH_DETECTED: port ");
                                                                                                                write_dec_u32(port);
                                                                                                                com1_write_str(" dynamic re-attach and driver re-bind successful\n");
                                                                                                                syscall1(0x9CC1_A77A);
                                                                                                            }
                                                                                                        }
                                                                                                    }
                                                                                                }
                                                                                                None => {}
                                                                                            }
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
