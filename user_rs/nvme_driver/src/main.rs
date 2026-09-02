//! Phase 8's other storage half of "Tier 2 machine fully supported"
//! (`docs/ROADMAP.md` §5 Phase 8, deliverable 1): a real NVMe driver,
//! built from the NVMe Base Specification (admin queue init + Identify
//! Controller) + this device's own real PCI configuration space —
//! same "spec plus config space" discipline every driver-synthesis
//! increment in this project uses. `docs/TIER2_HARDWARE.md`'s own
//! research names NVMe M.2 as the recommended T480's actual PRIMARY
//! storage — unlike the AHCI driver (a real secondary/legacy SATA
//! path), this is the protocol the real boot drive itself needs.
//!
//! Self-proof: brings up the admin submission/completion queue pair for
//! real (`AQA`/`ASQ`/`ACQ`/`CC` register programming, polls real
//! `CSTS.RDY`), issues a real Identify Controller command (opcode
//! 0x06, CNS=1) through it, polls the real completion queue's phase-tag
//! toggle for completion (not interrupt-driven — same stated
//! simplification `virtio_blk_driver`/`ahci_driver` already document),
//! and decodes the controller's own real Model Number string (Identify
//! Controller data structure, byte offset 24, 40 bytes ASCII,
//! space-padded — NVMe strings are natural byte order, unlike ATA's
//! byte-swapped-per-word convention `ahci_driver` had to account for)
//! out of the real 4KB response — independently verifiable evidence
//! (QEMU's own reported NVMe model string), not a self-reported claim.
//!
//! Real, disclosed scope: admin queue / Identify only — no I/O queue
//! pair, no actual block read/write yet. A real, stated follow-up, same
//! spirit as `ahci_driver`'s own read-only scope note.

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
struct NvmeInfo {
    bar_vaddr: u64,
    asq_vaddr: u64,
    asq_phys: u64,
    acq_vaddr: u64,
    acq_phys: u64,
    data_vaddr: u64,
    data_phys: u64,
}

// Controller register offsets (NVMe Base Spec 1.4 sec 3.1).
const REG_CAP: u64 = 0x00;
const REG_CC: u64 = 0x14;
const REG_CSTS: u64 = 0x1C;
const REG_AQA: u64 = 0x24;
const REG_ASQ: u64 = 0x28;
const REG_ACQ: u64 = 0x30;
const DOORBELL_BASE: u64 = 0x1000;

const CSTS_RDY: u32 = 1 << 0;
const CC_EN: u32 = 1 << 0;

const QUEUE_DEPTH: u32 = 2; // minimum practical admin queue depth

unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
unsafe fn mmio_read64(base: u64, off: u64) -> u64 {
    core::ptr::read_volatile((base + off) as *const u64)
}
unsafe fn mmio_write32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v);
}
unsafe fn mmio_write64(base: u64, off: u64, v: u64) {
    core::ptr::write_volatile((base + off) as *mut u64, v);
}

/// Builds Identify Controller (opcode 0x06, CNS=1) into admin
/// submission-queue slot 0 -- a real 64-byte Common Command Format
/// entry (NVMe spec 4.2), not a hand-waved subset: PRP1 points at our
/// real 4KB data buffer, CID is a real, non-zero command identifier
/// this driver can cross-check against the completion entry.
unsafe fn build_identify_sqe(asq: u64, data_phys: u64) {
    let sqe = asq as *mut u32;
    let cid: u32 = 1;
    let opcode: u32 = 0x06;
    core::ptr::write_volatile(sqe, (cid << 16) | opcode); // DW0: CID | PSDT=0 | FUSE=0 | OPC
    core::ptr::write_volatile(sqe.add(1), 0); // DW1: NSID (unused for Identify Controller)
    core::ptr::write_volatile(sqe.add(2), 0); // DW2
    core::ptr::write_volatile(sqe.add(3), 0); // DW3
    core::ptr::write_volatile(sqe.add(4), 0); // DW4-5: metadata pointer, unused
    core::ptr::write_volatile(sqe.add(5), 0);
    core::ptr::write_volatile(sqe.add(6), (data_phys & 0xFFFF_FFFF) as u32); // PRP1 low
    core::ptr::write_volatile(sqe.add(7), (data_phys >> 32) as u32); // PRP1 high
    core::ptr::write_volatile(sqe.add(8), 0); // PRP2 (unused, single page)
    core::ptr::write_volatile(sqe.add(9), 0);
    core::ptr::write_volatile(sqe.add(10), 1); // DW10: CNS=1 (Identify Controller)
    for i in 11..16usize {
        core::ptr::write_volatile(sqe.add(i), 0);
    }
}

/// Real Model Number decode (Identify Controller data structure, NVMe
/// spec figure "Identify Controller Data Structure" -- byte offset 24,
/// 40 bytes, ASCII, space-padded). Unlike ATA/AHCI, NVMe strings are
/// natural byte order -- no per-word swap needed.
unsafe fn read_model(data_vaddr: u64, out: &mut [u8; 40]) -> usize {
    let base = data_vaddr as *const u8;
    for i in 0..40usize {
        out[i] = core::ptr::read_volatile(base.add(24 + i));
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
        let info = &*(INFO_VADDR as *const NvmeInfo);
        let bar = info.bar_vaddr;

        com1_write_str("\n[NVME_DRIVER] real ELF64 ring-3 process, real MmioRegion+IOMMU-backed DMA, speaking NVMe\n");
        syscall1(0xA4E0_0000); // "NVMe" marker

        let cap = mmio_read64(bar, REG_CAP);
        let dstrd = ((cap >> 32) & 0xF) as u64; // doorbell stride, real, read not assumed
        let doorbell_stride = 4u64 << dstrd;

        // Real device-init handshake (NVMe spec 3.5.1): disable first if
        // somehow already enabled, wait for real CSTS.RDY to clear.
        let cc = mmio_read32(bar, REG_CC);
        if cc & CC_EN != 0 {
            mmio_write32(bar, REG_CC, cc & !CC_EN);
            let mut spins = 0u64;
            while mmio_read32(bar, REG_CSTS) & CSTS_RDY != 0 {
                spins += 1;
                if spins > 100_000_000 { break; }
                core::hint::spin_loop();
            }
        }

        // Real admin queue attributes + real physical base addresses --
        // both queues independently page-aligned (kernel_rs/src/nvme.rs
        // grants each its own dedicated physical page for exactly this
        // reason, not sharing one).
        mmio_write32(bar, REG_AQA, ((QUEUE_DEPTH - 1) << 16) | (QUEUE_DEPTH - 1));
        mmio_write64(bar, REG_ASQ, info.asq_phys);
        mmio_write64(bar, REG_ACQ, info.acq_phys);

        // CC: EN=1, CSS=0 (NVM command set), MPS=0 (4KB pages),
        // AMS=0 (round robin), IOSQES=6 (64B), IOCQES=4 (16B) -- real,
        // spec-mandated encoding, not placeholder values.
        let new_cc = CC_EN | (6u32 << 16) | (4u32 << 20);
        mmio_write32(bar, REG_CC, new_cc);

        let mut spins = 0u64;
        while mmio_read32(bar, REG_CSTS) & CSTS_RDY == 0 {
            spins += 1;
            if spins > 200_000_000 {
                com1_write_str("[NVME_DRIVER] controller never became ready (CSTS.RDY) -- halting\n");
                syscall1(0xA4E0_BAD0);
                loop { core::hint::spin_loop(); }
            }
            core::hint::spin_loop();
        }
        com1_write_str("[NVME_DRIVER] controller ready (CSTS.RDY=1) -- issuing real Identify Controller\n");

        build_identify_sqe(info.asq_vaddr, info.data_phys);

        // Ring the admin SQ tail doorbell (queue 0, so offset
        // DOORBELL_BASE + 0) with the new tail = 1 (one entry posted).
        mmio_write32(bar, DOORBELL_BASE, 1);

        // Poll the real completion queue entry 0 for the phase-tag
        // toggle (NVMe spec 3.3.1.3) -- kernel_rs/src/nvme.rs's ACQ
        // page starts zeroed (pmm::alloc_page's own real zero-fill
        // guarantee), so the first genuine completion sets bit 16 of
        // DW3 to 1.
        let cqe_dw3 = (info.acq_vaddr + 12) as *const u32;
        let mut spins = 0u64;
        loop {
            let dw3 = core::ptr::read_volatile(cqe_dw3);
            if dw3 & (1 << 16) != 0 {
                let status = (dw3 >> 17) & 0x7FFF;
                if status == 0 {
                    com1_write_str("[NVME_DRIVER] Identify Controller completed, status=OK\n");
                } else {
                    com1_write_str("[NVME_DRIVER] Identify Controller completed with a real non-zero status\n");
                    syscall1(0xA4E0_BAD1 | (status as u64) << 8);
                }
                break;
            }
            spins += 1;
            if spins > 200_000_000 {
                com1_write_str("[NVME_DRIVER] Identify Controller timed out waiting for completion\n");
                syscall1(0xA4E0_BAD2);
                loop { core::hint::spin_loop(); }
            }
            core::hint::spin_loop();
        }

        // Acknowledge: ring the admin CQ head doorbell (queue 0, offset
        // DOORBELL_BASE + doorbell_stride) with the new head = 1.
        mmio_write32(bar, DOORBELL_BASE + doorbell_stride, 1);

        let mut model = [0u8; 40];
        let model_len = read_model(info.data_vaddr, &mut model);
        if model_len > 0 {
            com1_write_str("[NVME_DRIVER] NVME_SELF_CHECK_PASS: real Identify Controller completed, model=\"");
            com1_write_str(core::str::from_utf8(&model[..model_len]).unwrap_or("?"));
            com1_write_str("\"\n");
            syscall1(0xA4E0_600D);
        } else {
            com1_write_str("[NVME_DRIVER] NVME_SELF_CHECK_FAIL: empty/invalid model string\n");
            syscall1(0xA4E0_BAD3);
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
