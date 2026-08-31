//! Phase 4's first real user-space driver: virtio-blk, speaking the
//! actual virtio 1.0 "modern" PCI transport wire protocol -- real
//! feature negotiation, a real virtqueue (descriptor/avail/used rings),
//! real MMIO register writes to a real QEMU-emulated device, through a
//! real capability-gated MMIO mapping and a real IOMMU-backed DMA
//! buffer (see kernel_rs/src/virtio_blk.rs's module doc for the kernel
//! side of this).
//!
//! Self-proof, no external verification tooling needed: writes a known
//! 512-byte pattern to disk sector 1, ZEROES the in-memory buffer (so a
//! stale-memory false-positive is impossible), issues a real read of
//! the SAME sector back into that now-zeroed buffer, and compares byte
//! for byte. A match is only possible if the device genuinely persisted
//! and returned real written data.
//!
//! Real, disclosed scope: virtqueue completion is POLLED (spins on the
//! used-ring index), not interrupt-driven -- see this crate's own
//! module doc in virtio_blk.rs for why that's a stated simplification,
//! not a hidden shortcut.

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

/// Real bug found bringing this driver up (the actual root cause behind
/// the page fault this crate's own self-check first hit): SYSCALL/SYSRET
/// does NOT save/restore general-purpose registers the way an
/// interrupt/iretq does, and the kernel's syscall_dispatch is a normal
/// extern "C" fn free to clobber every System V caller-saved register
/// (rdi/rsi/rdx/rcx/r8-r11), not just the two (rcx/r11) the hardware
/// itself repurposes for the return address/flags. This crate is the
/// FIRST driver in this kernel whose code actually kept a value (the
/// MMIO base address, in rsi/r9) live across a syscall call -- every
/// prior driver's syscall wrapper had the identical incomplete clobber
/// list, it just never mattered because nothing after the call still
/// needed those registers. Manifested as a page fault at a wildly wrong
/// address: the compiler believed `common` (bar_vaddr + common_off,
/// computed before the call) was still valid in rsi/r9 after syscall1(),
/// but the kernel's own dispatch logic had already overwritten both.
/// Full clobber list now; the other three driver crates were fixed the
/// same way once this was root-caused, even though they had no observed
/// symptom -- the bug was real in all of them, just latent.
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
struct VirtioBlkInfo {
    bar_vaddr: u64,
    common_off: u32,
    notify_off: u32,
    notify_multiplier: u32,
    isr_off: u32,
    device_off: u32,
    dma_vaddr: u64,
    dma_phys: u64,
}

// Common cfg register offsets (virtio 1.0 spec §4.1.4.3).
const REG_DEVICE_FEATURE_SELECT: u64 = 0x00;
const REG_DEVICE_FEATURE: u64 = 0x04;
const REG_DRIVER_FEATURE_SELECT: u64 = 0x08;
const REG_DRIVER_FEATURE: u64 = 0x0C;
const REG_DEVICE_STATUS: u64 = 0x14;
const REG_QUEUE_SELECT: u64 = 0x16;
const REG_QUEUE_SIZE: u64 = 0x18;
const REG_QUEUE_ENABLE: u64 = 0x1C;
const REG_QUEUE_NOTIFY_OFF: u64 = 0x1E;
const REG_QUEUE_DESC: u64 = 0x20;
const REG_QUEUE_DRIVER: u64 = 0x28;
const REG_QUEUE_DEVICE: u64 = 0x30;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const QUEUE_SIZE: u16 = 4;

// Layout within the one DMA page kernel_rs/src/virtio_blk.rs grants.
const DESC_OFF: u64 = 0x000; // 4 * 16 = 64 bytes
const AVAIL_OFF: u64 = 0x040; // 4 + 2*4 = 12 bytes
const USED_OFF: u64 = 0x080; // 4 + 8*4 = 36 bytes
const REQ_HDR_OFF: u64 = 0x0C0; // 16 bytes
const DATA_OFF: u64 = 0x0D0; // 512 bytes
const STATUS_OFF: u64 = 0x2D0; // 1 byte

#[repr(C)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
unsafe fn mmio_write8(base: u64, off: u64, v: u8) {
    core::ptr::write_volatile((base + off) as *mut u8, v)
}
unsafe fn mmio_write16(base: u64, off: u64, v: u16) {
    core::ptr::write_volatile((base + off) as *mut u16, v)
}
unsafe fn mmio_write32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v)
}
unsafe fn mmio_write64(base: u64, off: u64, v: u64) {
    core::ptr::write_volatile((base + off) as *mut u64, v)
}
unsafe fn mmio_read8(base: u64, off: u64) -> u8 {
    core::ptr::read_volatile((base + off) as *const u8)
}
unsafe fn mmio_read16(base: u64, off: u64) -> u16 {
    core::ptr::read_volatile((base + off) as *const u16)
}

/// Submits one descriptor chain (header -> data -> status) and polls the
/// used ring until the device completes it. Returns the real status byte
/// the device wrote (0 = VIRTIO_BLK_S_OK).
unsafe fn submit_and_wait(
    common: u64,
    notify_base: u64,
    dma: u64,
    dma_phys: u64,
    sector: u64,
    write: bool,
) -> u8 {
    let desc = (dma + DESC_OFF) as *mut Desc;
    let hdr = (dma + REQ_HDR_OFF) as *mut u32; // {type, reserved, sector_lo, sector_hi}
    core::ptr::write_volatile(hdr, if write { VIRTIO_BLK_T_OUT } else { VIRTIO_BLK_T_IN });
    core::ptr::write_volatile(hdr.add(1), 0); // reserved
    core::ptr::write_volatile((dma + REQ_HDR_OFF + 8) as *mut u64, sector);

    core::ptr::write_volatile(
        desc,
        Desc { addr: dma_phys + REQ_HDR_OFF, len: 16, flags: DESC_F_NEXT, next: 1 },
    );
    core::ptr::write_volatile(
        desc.add(1),
        Desc {
            addr: dma_phys + DATA_OFF,
            len: 512,
            flags: DESC_F_NEXT | if write { 0 } else { DESC_F_WRITE },
            next: 2,
        },
    );
    core::ptr::write_volatile(
        desc.add(2),
        Desc { addr: dma_phys + STATUS_OFF, len: 1, flags: DESC_F_WRITE, next: 0 },
    );

    // avail ring: {flags:u16, idx:u16, ring:[u16;QUEUE_SIZE]}
    let avail_idx_ptr = (dma + AVAIL_OFF + 2) as *mut u16;
    let cur_avail_idx = core::ptr::read_volatile(avail_idx_ptr);
    let ring_slot = (dma + AVAIL_OFF + 4 + 2 * ((cur_avail_idx % QUEUE_SIZE) as u64)) as *mut u16;
    core::ptr::write_volatile(ring_slot, 0); // head descriptor index
    core::ptr::write_volatile(avail_idx_ptr, cur_avail_idx.wrapping_add(1));

    // Notify: tell the device to check the avail ring. queue_notify_off
    // was read once at setup and folded into notify_base by the caller.
    mmio_write16(notify_base, 0, 0); // queue index 0

    // Poll the used ring (real, stated simplification -- see module doc).
    let used_idx_ptr = (dma + USED_OFF + 2) as *const u16;
    let target = cur_avail_idx.wrapping_add(1);
    while core::ptr::read_volatile(used_idx_ptr) != target {
        core::hint::spin_loop();
    }

    let _ = common; // (kept for signature symmetry / future ISR-status reads)
    core::ptr::read_volatile((dma + STATUS_OFF) as *const u8)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        let info = &*(INFO_VADDR as *const VirtioBlkInfo);
        let bar = info.bar_vaddr;
        let common = bar + info.common_off as u64;
        let notify_base_region = bar + info.notify_off as u64;
        let dma = info.dma_vaddr;
        let dma_phys = info.dma_phys;

        com1_write_str("\n[VIRTIO_BLK_DRIVER] real ELF64 ring-3 process, real MmioRegion+IOMMU-backed DMA, speaking virtio 1.0\n");
        syscall1(0x81C0); // "bLoCk"-ish marker

        // Real device-init handshake (virtio 1.0 spec 3.1.1).
        mmio_write8(common, REG_DEVICE_STATUS, 0); // reset
        mmio_write8(common, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE);
        mmio_write8(common, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

        // Feature negotiation: accept ONLY VIRTIO_F_VERSION_1 (bit 32 --
        // feature_select=1, bit 0 of that dword). Required for a modern
        // device to proceed at all; every optional feature (indirect
        // descriptors, event index, etc.) deliberately left unnegotiated
        // to keep this first increment's protocol surface minimal.
        mmio_write32(common, REG_DEVICE_FEATURE_SELECT, 1);
        let _hi_features = mmio_read32(common, REG_DEVICE_FEATURE);
        mmio_write32(common, REG_DRIVER_FEATURE_SELECT, 0);
        mmio_write32(common, REG_DRIVER_FEATURE, 0);
        mmio_write32(common, REG_DRIVER_FEATURE_SELECT, 1);
        mmio_write32(common, REG_DRIVER_FEATURE, 1); // VERSION_1

        mmio_write8(common, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
        let status_check = mmio_read8(common, REG_DEVICE_STATUS);
        if status_check & STATUS_FEATURES_OK == 0 {
            com1_write_str("[VIRTIO_BLK_DRIVER] device rejected FEATURES_OK -- halting\n");
            loop {
                core::hint::spin_loop();
            }
        }

        // Queue 0 setup.
        mmio_write16(common, REG_QUEUE_SELECT, 0);
        mmio_write16(common, REG_QUEUE_SIZE, QUEUE_SIZE);
        mmio_write64(common, REG_QUEUE_DESC, dma_phys + DESC_OFF);
        mmio_write64(common, REG_QUEUE_DRIVER, dma_phys + AVAIL_OFF);
        mmio_write64(common, REG_QUEUE_DEVICE, dma_phys + USED_OFF);
        let notify_off_multiplier_reg = mmio_read16(common, REG_QUEUE_NOTIFY_OFF);
        mmio_write16(common, REG_QUEUE_ENABLE, 1);

        mmio_write8(common, REG_DEVICE_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK | STATUS_DRIVER_OK);

        let notify_base = notify_base_region + (notify_off_multiplier_reg as u64) * (info.notify_multiplier as u64);

        com1_write_str("[VIRTIO_BLK_DRIVER] device live (DRIVER_OK) -- issuing self-check write/read\n");

        // Real self-check: write a known pattern to sector 1, zero the
        // buffer, read it back, compare.
        let pattern: u8 = 0xB1;
        let data_ptr = (dma + DATA_OFF) as *mut u8;
        for i in 0..512usize {
            core::ptr::write_volatile(data_ptr.add(i), pattern ^ (i as u8));
        }
        let write_status = submit_and_wait(common, notify_base, dma, dma_phys, 1, true);
        syscall1(0x8200_0000 | write_status as u64);

        for i in 0..512usize {
            core::ptr::write_volatile(data_ptr.add(i), 0);
        }
        let read_status = submit_and_wait(common, notify_base, dma, dma_phys, 1, false);
        syscall1(0x8300_0000 | read_status as u64);

        let mut matched = true;
        for i in 0..512usize {
            if core::ptr::read_volatile(data_ptr.add(i)) != (pattern ^ (i as u8)) {
                matched = false;
                break;
            }
        }
        if write_status == 0 && read_status == 0 && matched {
            com1_write_str("[VIRTIO_BLK_DRIVER] SELF_CHECK_PASS: write+zero+read+compare all matched\n");
            syscall1(0x900D_600D);
        } else {
            com1_write_str("[VIRTIO_BLK_DRIVER] SELF_CHECK_FAIL\n");
            syscall1(0xBAD0_0000 | matched as u64);
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
