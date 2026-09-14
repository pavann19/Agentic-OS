//! Must stay field-for-field identical to `kernel_rs/src/bootinfo.rs` and
//! `include/bootinfo.h` — three independent copies of the same ABI contract
//! (C bootloader/kernel reference, Rust bootloader, Rust kernel). Changing
//! one without the other two breaks the handoff silently.

#![allow(dead_code)]

use core::ffi::c_void;

pub const BOOTINFO_MAGIC: u64 = 0x41474F53424F4F54; // "AGOSBOOT"
pub const BOOTINFO_VERSION: u32 = 1;
pub const PSF1_MAGIC: u16 = 0x0436;

#[repr(C)]
pub struct Psf1Header {
    pub magic: u16,
    pub mode: u8,
    pub chars_size: u8,
}

#[repr(C)]
pub struct Psf1Font {
    pub header: *mut Psf1Header,
    pub glyph_buffer: *mut c_void,
}

#[repr(C)]
pub struct Framebuffer {
    pub base_address: *mut c_void,
    pub buffer_size: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scan_line: u32,
}

// Reuses uefi::EfiMemoryDescriptor rather than declaring a second,
// identically-shaped struct here — same-crate duplicate nominal types don't
// unify in Rust even with identical layout (see the build error this
// replaced). kernel_rs keeps its own independent copy; that symmetry is
// unaffected — this dedup is purely internal to boot_rs.
use crate::uefi::EfiMemoryDescriptor;

#[repr(C)]
pub struct BootInfoPayload {
    pub framebuffer: *mut Framebuffer,
    pub font: *mut Psf1Font,
    pub memory_map: *mut EfiMemoryDescriptor,
    pub memory_map_size: u64,
    pub memory_map_descriptor_size: u64,
    pub memory_map_descriptor_version: u32,
    pub rsdp: *mut c_void,
    pub kernel_physical_start: u64,
    pub kernel_physical_end: u64,
    pub kernel_virtual_base: u64,
    pub kernel_hash: [u8; 32],
    pub tpm_log: *const c_void,
}

#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u32,
    pub size: u32,
    pub payload: BootInfoPayload,
}
