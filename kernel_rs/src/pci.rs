//! PCIe enumeration. Phase 3 item — `docs/ROADMAP.md` frames this as a
//! user-space service eventually; this builds the real mechanism at
//! kernel level first (matching how every other Phase 2/3 primitive in
//! this kernel was built: prove the mechanism, then expose it), since a
//! user-space PCI enumerator needs `driver.rs`'s port-I/O capability
//! grant for ports 0xCF8/0xCFC to exist and be provably correct before
//! there's anything for it to be granted access TO.
//!
//! Uses PCI Configuration Mechanism #1 (the CF8/CFC I/O ports) — works
//! for accessing the first 256 bytes of any PCIe device's config space
//! too (PCIe's real config space is memory-mapped and larger, via MCFG/
//! ECAM, but the legacy mechanism is universally supported as a
//! backward-compatible subset and is what every device this kernel has
//! actually needed to enumerate so far responds to correctly).

use crate::klog_info;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}

unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    value
}

fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    (1 << 31)
        | ((bus as u32) << 16)
        | (((device & 0x1F) as u32) << 11)
        | (((function & 0x07) as u32) << 8)
        | ((offset & 0xFC) as u32)
}

fn read_config_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, config_address(bus, device, function, offset));
        inl(CONFIG_DATA)
    }
}

fn read_config_u16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let dword = read_config_u32(bus, device, function, offset & 0xFC);
    let shift = (offset & 2) * 8;
    ((dword >> shift) & 0xFFFF) as u16
}

fn read_config_u8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let dword = read_config_u32(bus, device, function, offset & 0xFC);
    let shift = (offset & 3) * 8;
    ((dword >> shift) & 0xFF) as u8
}

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

/// Full brute-force scan (256 buses x 32 devices x 8 functions) — no
/// bridge-topology-aware recursion yet, the same tradeoff every simple
/// PCI enumerator makes before it grows one; correct, just not the most
/// efficient possible scan. Returns every device found, vendor_id 0xFFFF
/// (the "not present" sentinel Configuration Mechanism #1 returns for any
/// address with nothing there) filtered out already.
pub fn enumerate() -> alloc::vec::Vec<PciDevice> {
    let mut found = alloc::vec::Vec::new();
    for bus in 0..=255u16 {
        let bus = bus as u8;
        for device in 0..32u8 {
            let vendor_id = read_config_u16(bus, device, 0, 0x00);
            if vendor_id == 0xFFFF {
                continue;
            }
            let header_type = read_config_u8(bus, device, 0, 0x0E);
            let multi_function = header_type & 0x80 != 0;
            let max_function = if multi_function { 8 } else { 1 };
            for function in 0..max_function {
                let vendor_id = read_config_u16(bus, device, function, 0x00);
                if vendor_id == 0xFFFF {
                    continue;
                }
                let device_id = read_config_u16(bus, device, function, 0x02);
                let class = read_config_u8(bus, device, function, 0x0B);
                let subclass = read_config_u8(bus, device, function, 0x0A);
                let prog_if = read_config_u8(bus, device, function, 0x09);
                let ht = read_config_u8(bus, device, function, 0x0E);
                found.push(PciDevice {
                    bus,
                    device,
                    function,
                    vendor_id,
                    device_id,
                    class,
                    subclass,
                    prog_if,
                    header_type: ht,
                });
            }
        }
    }
    found
}

pub fn log_all(devices: &[PciDevice]) {
    for d in devices {
        klog_info!(
            "PCI {:02x}:{:02x}.{} vendor=0x{:04x} device=0x{:04x} class=0x{:02x} subclass=0x{:02x} prog_if=0x{:02x}",
            d.bus, d.device, d.function, d.vendor_id, d.device_id, d.class, d.subclass, d.prog_if
        );
    }
}
