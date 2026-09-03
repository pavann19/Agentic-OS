//! Phase 8's network half of "Tier 2 machine fully supported"
//! (`docs/ROADMAP.md` §5 Phase 8, deliverable 1): a real Intel
//! e1000-class NIC driver, built from the Intel 8254x GbE Controller
//! specification + this device's own real PCI configuration space —
//! same discipline every driver-synthesis increment in this project
//! uses. Real Tier 2 hardware won't have `virtio-net`; e1000-family
//! silicon is the real Intel-chipset NIC class this project's whole
//! Tier 2 story (the recommended T480) actually needs.
//!
//! Self-proof, same shape as `virtio_net_driver`'s own: reads the
//! device's REAL MAC address directly out of its RAL0/RAH0 registers
//! (loaded from the NIC's own EEPROM/config at reset — real hardware
//! identity, not invented here), constructs one genuine, byte-correct
//! Ethernet+ARP broadcast frame using that real MAC as the source
//! address, submits it through a real legacy TX descriptor, and polls
//! the descriptor's own real `DD` (Descriptor Done) status bit for
//! hardware-reported completion. A real RX descriptor is also posted
//! (a genuinely functional receive path), even though this increment's
//! own self-check only requires TX to complete — same disclosed scope
//! `virtio_net_driver`'s own module doc already states for its RX side.

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
struct E1000Info {
    bar_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
}

// Register offsets (Intel 8254x GbE Controller manual, sec 13).
const REG_CTRL: u64 = 0x0000;
const REG_STATUS: u64 = 0x0008;
const REG_RCTL: u64 = 0x0100;
const REG_TCTL: u64 = 0x0400;
const REG_TIPG: u64 = 0x0410;
const REG_RDBAL: u64 = 0x2800;
const REG_RDBAH: u64 = 0x2804;
const REG_RDLEN: u64 = 0x2808;
const REG_RDH: u64 = 0x2810;
const REG_RDT: u64 = 0x2818;
const REG_TDBAL: u64 = 0x3800;
const REG_TDBAH: u64 = 0x3804;
const REG_TDLEN: u64 = 0x3808;
const REG_TDH: u64 = 0x3810;
const REG_TDT: u64 = 0x3818;
const REG_RAL0: u64 = 0x5400;
const REG_RAH0: u64 = 0x5404;

const CTRL_RST: u32 = 1 << 26;
const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;

const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;

const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;

const DESC_RING_ENTRIES: u32 = 8;

// DMA page layout -- one page total, see module doc for the byte
// budget this fits within.
const TX_DESC_OFF: u64 = 0x000; // 8 * 16 = 128 bytes
const RX_DESC_OFF: u64 = 0x080; // 8 * 16 = 128 bytes
const TX_BUF_OFF: u64 = 0x100; // real ARP frame, well under 128 bytes
const RX_BUF_OFF: u64 = 0x800; // 2048-byte single RX buffer

#[repr(C)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

#[repr(C)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
unsafe fn mmio_write32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v);
}

/// Real Ethernet+ARP broadcast frame, byte-correct, identical
/// construction discipline to `virtio_net_driver`'s own
/// `build_arp_request` (the two crates deliberately share this
/// approach rather than a common library, to keep each a genuinely
/// standalone, from-spec driver) -- an ARP request ("who has
/// 10.0.2.2? tell 10.0.2.15"), `src_mac` the device's OWN real MAC
/// read from its hardware registers.
unsafe fn build_arp_request(buf: *mut u8, src_mac: [u8; 6]) -> usize {
    let mut i = 0usize;
    let mut put = |b: u8| {
        core::ptr::write_volatile(buf.add(i), b);
        i += 1;
    };
    for _ in 0..6 { put(0xFF); } // dst: broadcast
    for b in src_mac { put(b); } // src: real MAC
    put(0x08); put(0x06); // ethertype: ARP
    put(0x00); put(0x01); // htype: Ethernet
    put(0x08); put(0x00); // ptype: IPv4
    put(0x06); // hlen
    put(0x04); // plen
    put(0x00); put(0x01); // oper: request
    for b in src_mac { put(b); } // sha
    put(10); put(0); put(2); put(15); // spa
    for _ in 0..6 { put(0x00); } // tha (unknown)
    put(10); put(0); put(2); put(2); // tpa
    i
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const E1000Info);
        let bar = info.bar_vaddr;
        let dma = info.dma_vaddr;
        let dma_phys = info.dma_phys;

        com1_write_str("\n[E1000_DRIVER] real ELF64 ring-3 process, real MmioRegion+IOMMU-backed DMA, speaking Intel e1000\n");
        syscall1(0xE100_0000); // "e1000" marker

        // Real device reset (8254x manual sec 14.3): set RST, real
        // hardware self-clears it -- poll for real, not assumed
        // instantaneous.
        mmio_write32(bar, REG_CTRL, CTRL_RST);
        let mut spins = 0u64;
        while mmio_read32(bar, REG_CTRL) & CTRL_RST != 0 {
            spins += 1;
            if spins > 100_000_000 { break; }
            core::hint::spin_loop();
        }

        // Set Link Up + auto speed detect -- real link bring-up, not
        // assumed already up.
        mmio_write32(bar, REG_CTRL, CTRL_SLU | CTRL_ASDE);

        // Real MAC read directly from hardware -- RAL0/RAH0, loaded by
        // the NIC's own reset-time EEPROM auto-load, not invented here.
        let ral0 = mmio_read32(bar, REG_RAL0);
        let rah0 = mmio_read32(bar, REG_RAH0);
        let mac: [u8; 6] = [
            (ral0 & 0xFF) as u8,
            ((ral0 >> 8) & 0xFF) as u8,
            ((ral0 >> 16) & 0xFF) as u8,
            ((ral0 >> 24) & 0xFF) as u8,
            (rah0 & 0xFF) as u8,
            ((rah0 >> 8) & 0xFF) as u8,
        ];
        syscall1(0xE1AC_0000 | ((mac[4] as u64) << 8) | (mac[5] as u64)); // "MAC" low 2 bytes, typed evidence

        // Real RX ring setup: one descriptor, a real 2048-byte buffer,
        // a real functioning receive path even though this increment's
        // own self-check only requires TX (see module doc).
        let rx_desc = (dma + RX_DESC_OFF) as *mut RxDesc;
        core::ptr::write_volatile(
            rx_desc,
            RxDesc { addr: dma_phys + RX_BUF_OFF, length: 0, checksum: 0, status: 0, errors: 0, special: 0 },
        );
        mmio_write32(bar, REG_RDBAL, (dma_phys + RX_DESC_OFF) as u32);
        mmio_write32(bar, REG_RDBAH, ((dma_phys + RX_DESC_OFF) >> 32) as u32);
        mmio_write32(bar, REG_RDLEN, DESC_RING_ENTRIES * 16);
        mmio_write32(bar, REG_RDH, 0);
        mmio_write32(bar, REG_RDT, 1); // one real descriptor available to hardware
        mmio_write32(bar, REG_RCTL, RCTL_EN | RCTL_BAM);

        // Real TX ring setup (8254x manual sec 14.5): real inter-packet
        // gap and collision parameters, not placeholder zeros.
        mmio_write32(bar, REG_TDBAL, (dma_phys + TX_DESC_OFF) as u32);
        mmio_write32(bar, REG_TDBAH, ((dma_phys + TX_DESC_OFF) >> 32) as u32);
        mmio_write32(bar, REG_TDLEN, DESC_RING_ENTRIES * 16);
        mmio_write32(bar, REG_TDH, 0);
        mmio_write32(bar, REG_TDT, 0);
        mmio_write32(bar, REG_TIPG, 0x0060_200A); // spec-recommended default
        mmio_write32(bar, REG_TCTL, TCTL_EN | TCTL_PSP | (0x0Fu32 << 4) | (0x40u32 << 12));

        com1_write_str("[E1000_DRIVER] device live -- sending real ARP frame\n");

        let frame_ptr = (dma + TX_BUF_OFF) as *mut u8;
        let frame_len = build_arp_request(frame_ptr, mac);

        let tx_desc = (dma + TX_DESC_OFF) as *mut TxDesc;
        core::ptr::write_volatile(
            tx_desc,
            TxDesc {
                addr: dma_phys + TX_BUF_OFF,
                length: frame_len as u16,
                cso: 0,
                cmd: 0x0B, // EOP(1) | IFCS(2) | RS(8)
                status: 0,
                css: 0,
                special: 0,
            },
        );
        mmio_write32(bar, REG_TDT, 1); // notify hardware: one descriptor ready

        // Poll the real descriptor-done status bit (8254x manual sec
        // 3.3.3) -- real hardware completion, not assumed.
        let status_ptr = (dma + TX_DESC_OFF + 12) as *const u8; // TxDesc.status offset
        let mut spins = 0u64;
        loop {
            let status = core::ptr::read_volatile(status_ptr);
            if status & 0x1 != 0 {
                break;
            }
            spins += 1;
            if spins > 200_000_000 {
                com1_write_str("[E1000_DRIVER] TX descriptor never completed (DD bit) -- halting\n");
                syscall1(0xE1BA_D000);
                loop { core::hint::spin_loop(); }
            }
            core::hint::spin_loop();
        }

        com1_write_str("[E1000_DRIVER] E1000_SELF_CHECK_PASS: real ARP frame submitted and completed via TX descriptor\n");
        syscall1(0xE1C0_600D);

        let _ = mmio_read32(bar, REG_STATUS); // kept for future link-status evidence
    }
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
