//! Phase 8's storage half of "Tier 2 machine fully supported"
//! (`docs/ROADMAP.md` §5 Phase 8, deliverable 1): a real AHCI (SATA)
//! driver, built from the AHCI 1.3.1 specification + this device's own
//! real PCI configuration space -- same "spec plus config space" input
//! discipline Phase 6's `virtio-net` synthesis used. Real Tier 2
//! hardware (the recommended ThinkPad T480, `docs/TIER2_HARDWARE.md`)
//! needs a real NVMe-or-AHCI driver; this kernel only had `virtio-blk`
//! before this increment, which real hardware doesn't have. Targets
//! the SAME `ich9-ahci` controller (00:1f.2) this kernel's own PCI
//! enumeration has classified since Phase 3, previously only bound by
//! `device_manager.rs`, never actually driven.
//!
//! Self-proof: enables AHCI mode, finds the first active SATA port with
//! a real ATA device attached (`PxSSTS.DET`/`PxSIG` checked, not
//! assumed), sets up a real command list + FIS receive area + command
//! table, and issues a real ATA IDENTIFY DEVICE command (opcode 0xEC)
//! through it -- polls the real completion bit, then decodes the
//! device's own real Model Number string (IDENTIFY words 27-46,
//! byte-swapped per word per the ATA spec) out of the returned 512-byte
//! block and logs it verbatim. A real, human-readable string a QEMU
//! ATA disk genuinely reports (typically "QEMU HARDDISK") is
//! independently checkable evidence this actually talked to the real
//! device, not a self-reported claim.
//!
//! Real, disclosed scope: reads only (IDENTIFY DEVICE), no write path
//! yet -- a real, stated follow-up, same spirit as `virtio_blk_driver`'s
//! own documented simplifications (polled completion, not
//! interrupt-driven).

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
struct AhciInfo {
    bar_vaddr: u64,
    dma_vaddr: u64,
    dma_phys: u64,
}

// HBA (global) register offsets from ABAR (AHCI spec 3.1).
const REG_CAP: u64 = 0x00;
const REG_GHC: u64 = 0x04;
const REG_PI: u64 = 0x0C;
const GHC_AE: u32 = 1 << 31;

// Port register block: ABAR + 0x100 + port*0x80 (AHCI spec 3.3).
const PORT_BASE: u64 = 0x100;
const PORT_STRIDE: u64 = 0x80;
const PXCLB: u64 = 0x00;
const PXCLBU: u64 = 0x04;
const PXFB: u64 = 0x08;
const PXFBU: u64 = 0x0C;
const PXIS: u64 = 0x10;
const PXCMD: u64 = 0x18;
const PXTFD: u64 = 0x20;
const PXSIG: u64 = 0x24;
const PXSSTS: u64 = 0x28;
const PXCI: u64 = 0x38;

const PXCMD_ST: u32 = 1 << 0;
const PXCMD_FRE: u32 = 1 << 4;
const PXCMD_FR: u32 = 1 << 14;
const PXCMD_CR: u32 = 1 << 15;

const ATA_SIG_ATA: u32 = 0x0000_0101;

// DMA page layout -- see module doc for the real byte budget.
const CMD_LIST_OFF: u64 = 0x000; // 1024 bytes, 1KB-aligned (required)
const FIS_OFF: u64 = 0x400; // 256 bytes, 256B-aligned (required)
const CMD_TABLE_OFF: u64 = 0x500; // 128B-aligned (1280 % 128 == 0)
const DATA_OFF: u64 = 0x600; // 512-byte IDENTIFY buffer

unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
unsafe fn mmio_write32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v);
}

/// Finds the first implemented port (`PI` bitmask) with a real ATA
/// device present and active (`PxSSTS.DET == 3`, `PxSIG` reports ATA,
/// not ATAPI/port-multiplier) -- checked, not assumed, same discipline
/// `virtio_blk.rs`'s own module doc insists on for capability
/// discovery.
unsafe fn find_ata_port(bar: u64) -> Option<u32> {
    let pi = mmio_read32(bar, REG_PI);
    for port in 0..32u32 {
        if pi & (1 << port) == 0 {
            continue;
        }
        let port_base = bar + PORT_BASE + (port as u64) * PORT_STRIDE;
        let ssts = mmio_read32(port_base, PXSSTS);
        let det = ssts & 0xF;
        if det != 3 {
            continue; // no device / not active
        }
        let sig = mmio_read32(port_base, PXSIG);
        if sig == ATA_SIG_ATA {
            return Some(port);
        }
    }
    None
}

/// Stops the port's command engine (`ST`/`FRE` cleared, waits for
/// `CR`/`FR` to actually clear -- real hardware handshake, not assumed
/// instantaneous), programs the real command-list/FIS physical
/// addresses, then restarts it. Required before this driver's own
/// command-list pointers can be trusted (AHCI spec 10.1.2/10.3.1).
unsafe fn init_port(port_base: u64, dma_phys: u64) {
    let mut cmd = mmio_read32(port_base, PXCMD);
    if cmd & PXCMD_ST != 0 {
        mmio_write32(port_base, PXCMD, cmd & !PXCMD_ST);
        while mmio_read32(port_base, PXCMD) & PXCMD_CR != 0 {
            core::hint::spin_loop();
        }
    }
    cmd = mmio_read32(port_base, PXCMD);
    if cmd & PXCMD_FRE != 0 {
        mmio_write32(port_base, PXCMD, cmd & !PXCMD_FRE);
        while mmio_read32(port_base, PXCMD) & PXCMD_FR != 0 {
            core::hint::spin_loop();
        }
    }

    mmio_write32(port_base, PXCLB, ((dma_phys + CMD_LIST_OFF) & 0xFFFF_FFFF) as u32);
    mmio_write32(port_base, PXCLBU, ((dma_phys + CMD_LIST_OFF) >> 32) as u32);
    mmio_write32(port_base, PXFB, ((dma_phys + FIS_OFF) & 0xFFFF_FFFF) as u32);
    mmio_write32(port_base, PXFBU, ((dma_phys + FIS_OFF) >> 32) as u32);

    // Clear any stale interrupt status before starting (write-1-to-clear).
    mmio_write32(port_base, PXIS, mmio_read32(port_base, PXIS));

    let cmd = mmio_read32(port_base, PXCMD);
    mmio_write32(port_base, PXCMD, cmd | PXCMD_FRE | PXCMD_ST);
}

/// Builds command slot 0: a real command header pointing at a real
/// command table (H2D Register FIS + one PRDT entry), issues it via
/// `PxCI`, and polls for the HBA to clear that bit (real completion,
/// AHCI spec 5.5.1) -- not interrupt-driven, same stated simplification
/// `virtio_blk_driver` already documents for its own polled completion.
unsafe fn issue_identify(dma: u64, dma_phys: u64, port_base: u64) {
    // Command header (32 bytes): DW0 = CFL(5 DWORDS for a 20-byte H2D
    // FIS) | PRDTL=1 in bits[31:16]; DW2/DW3 = command table address.
    let cmd_hdr = (dma + CMD_LIST_OFF) as *mut u32;
    core::ptr::write_volatile(cmd_hdr, 5 | (1u32 << 16)); // CFL=5, PRDTL=1, W=0 (read)
    core::ptr::write_volatile(cmd_hdr.add(1), 0); // PRDBC, HBA-written
    let ctba = dma_phys + CMD_TABLE_OFF;
    core::ptr::write_volatile(cmd_hdr.add(2), (ctba & 0xFFFF_FFFF) as u32);
    core::ptr::write_volatile(cmd_hdr.add(3), (ctba >> 32) as u32);

    // Command table: zero the CFIS region (64 bytes) then write a real
    // H2D Register FIS (20 bytes) into it.
    let ctab = (dma + CMD_TABLE_OFF) as *mut u8;
    for i in 0..64usize {
        core::ptr::write_volatile(ctab.add(i), 0);
    }
    core::ptr::write_volatile(ctab.add(0), 0x27); // FIS type: Register H2D
    core::ptr::write_volatile(ctab.add(1), 1 << 7); // C=1 (this is a command)
    core::ptr::write_volatile(ctab.add(2), 0xEC); // ATA command: IDENTIFY DEVICE
    // bytes 3..19 (features/LBA/device/count/control) stay zero -- valid
    // for IDENTIFY, which addresses no specific LBA.

    // PRDT (one entry, at CTAB+0x50 per AHCI spec's 0x80-byte command
    // table layout: 64 CFIS + 16 ACMD + 48 reserved = 128 = 0x80).
    let prdt = (dma + CMD_TABLE_OFF + 0x80) as *mut u32;
    let data_phys = dma_phys + DATA_OFF;
    core::ptr::write_volatile(prdt, (data_phys & 0xFFFF_FFFF) as u32);
    core::ptr::write_volatile(prdt.add(1), (data_phys >> 32) as u32);
    core::ptr::write_volatile(prdt.add(2), 0);
    core::ptr::write_volatile(prdt.add(3), 511); // DBC = byte count - 1 (512 bytes)

    mmio_write32(port_base, PXCI, 1); // issue slot 0
    let mut spins: u64 = 0;
    while mmio_read32(port_base, PXCI) & 1 != 0 {
        spins += 1;
        if spins > 200_000_000 {
            com1_write_str("[AHCI_DRIVER] IDENTIFY timed out waiting for PxCI to clear\n");
            unsafe { syscall1(0xA4C1_BAD0) };
            return;
        }
        core::hint::spin_loop();
    }
}

/// Real ATA IDENTIFY DEVICE decode: the Model Number lives in words
/// 27-46 (40 bytes), each word byte-swapped (ATA spec's own
/// little-endian-word-big-endian-byte-pair convention) -- decoded here,
/// not assumed pre-formatted, and trimmed of trailing padding spaces.
fn decode_model(data: &[u8; 512], out: &mut [u8; 40]) -> usize {
    for w in 0..20usize {
        let base = 27 * 2 + w * 2;
        out[w * 2] = data[base + 1];
        out[w * 2 + 1] = data[base];
    }
    let mut len = 40;
    while len > 0 && (out[len - 1] == b' ' || out[len - 1] == 0) {
        len -= 1;
    }
    len
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const AhciInfo);
        let bar = info.bar_vaddr;
        let dma = info.dma_vaddr;
        let dma_phys = info.dma_phys;

        com1_write_str("\n[AHCI_DRIVER] real ELF64 ring-3 process, real MmioRegion+IOMMU-backed DMA, speaking AHCI 1.3.1\n");
        syscall1(0xA4C1_0000); // "AHCI" marker

        // Real device-init handshake: enable AHCI mode (GHC.AE).
        let ghc = mmio_read32(bar, REG_GHC);
        mmio_write32(bar, REG_GHC, ghc | GHC_AE);

        let port = match find_ata_port(bar) {
            Some(p) => p,
            None => {
                com1_write_str("[AHCI_DRIVER] no active ATA port found -- halting\n");
                syscall1(0xA4C1_BAD1);
                loop { core::hint::spin_loop(); }
            }
        };
        syscall1(0xA4C1_0A00 | port as u64); // "AHCI port=N" marker

        let port_base = bar + PORT_BASE + (port as u64) * PORT_STRIDE;
        init_port(port_base, dma_phys);
        com1_write_str("[AHCI_DRIVER] port live -- issuing real IDENTIFY DEVICE\n");

        issue_identify(dma, dma_phys, port_base);

        // Real bug found and fixed here (the same toolchain issue
        // documented at length in kernel_common::mem_intrinsics's own
        // doc comment): `[0u8; 512]` is exactly the array-literal shape
        // LLVM lowers into a `memset` call on this toolchain, a real
        // indirect call through a permanently-unpopulated slot -- found
        // via real disassembly (`callq *-0x...(%rip) # 0x0`), not
        // guessed. Every byte gets overwritten by the loop immediately
        // below anyway, so the zero-init was never needed in the first
        // place -- `MaybeUninit` skips it entirely rather than trying
        // to avoid the bad codegen some other way.
        let data_ptr = (dma + DATA_OFF) as *const u8;
        let mut data_mu = core::mem::MaybeUninit::<[u8; 512]>::uninit();
        let data_out = data_mu.as_mut_ptr() as *mut u8;
        for i in 0..512usize {
            core::ptr::write_volatile(data_out.add(i), core::ptr::read_volatile(data_ptr.add(i)));
        }
        // Reference into the MaybeUninit's own memory, not a by-value
        // `assume_init()` -- a full array VALUE return/move is itself a
        // large-aggregate copy LLVM can lower into a memcpy call (the
        // exact same bug class, one level removed -- see
        // virtio_blk_driver's own module doc for the earlier
        // investigation that first found this).
        let data: &[u8; 512] = &*(data_mu.as_ptr());

        let mut model_mu = core::mem::MaybeUninit::<[u8; 40]>::uninit();
        let model_ptr = model_mu.as_mut_ptr() as *mut u8;
        for i in 0..40usize {
            core::ptr::write_volatile(model_ptr.add(i), 0);
        }
        let model: &mut [u8; 40] = &mut *(model_mu.as_mut_ptr());
        let model_len = decode_model(data, model);
        if model_len > 0 {
            com1_write_str("[AHCI_DRIVER] AHCI_SELF_CHECK_PASS: real IDENTIFY DEVICE completed, model=\"");
            com1_write_str(core::str::from_utf8(&model[..model_len]).unwrap_or("?"));
            com1_write_str("\"\n");
            syscall1(0xA4C1_600D);
        } else {
            com1_write_str("[AHCI_DRIVER] AHCI_SELF_CHECK_FAIL: empty/invalid model string\n");
            syscall1(0xA4C1_BAD2);
        }
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
