//! Mirrors `include/bootinfo.h` field-for-field. The bootloader (`boot/main.c`)
//! is still C and constructs this struct; this file is the Rust-side contract
//! for reading it. Any field reorder on either side breaks the ABI silently,
//! so this file's layout must be changed in lockstep with bootinfo.h.

#![allow(dead_code)]

pub const BOOTINFO_MAGIC: u64 = 0x41474F53424F4F54; // "AGOSBOOT"
pub const BOOTINFO_VERSION: u32 = 1;

#[repr(C)]
pub struct Psf1Header {
    pub magic: u16,
    pub mode: u8,
    pub chars_size: u8,
}

#[repr(C)]
pub struct Psf1Font {
    pub header: *mut Psf1Header,
    pub glyph_buffer: *mut core::ffi::c_void,
}

#[repr(C)]
pub struct Framebuffer {
    pub base_address: *mut core::ffi::c_void,
    pub buffer_size: u64,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scan_line: u32,
}

#[repr(C)]
pub struct EfiMemoryDescriptor {
    pub ty: u32,
    pub pad: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

#[repr(C)]
pub struct BootInfoPayload {
    pub framebuffer: *mut Framebuffer,
    pub font: *mut Psf1Font,
    pub memory_map: *mut EfiMemoryDescriptor,
    pub memory_map_size: u64,
    pub memory_map_descriptor_size: u64,
    pub memory_map_descriptor_version: u32,
    pub rsdp: *mut core::ffi::c_void,
    pub kernel_physical_start: u64,
    pub kernel_physical_end: u64,
    pub kernel_virtual_base: u64,
    pub kernel_hash: [u8; 32],
    pub tpm_log: *const core::ffi::c_void,
}

#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u32,
    pub size: u32,
    pub payload: BootInfoPayload,
}

/// Validates magic/version the same way `kernel/kernel.c:15` currently does.
/// Does NOT yet validate `size` against `core::mem::size_of::<BootInfo>()` —
/// that check is a Phase 0 TODO once the struct is confirmed layout-stable
/// against the C side under the actual cross compiler (see PHASE0_PROGRESS.md).
pub unsafe fn validate(info: *const BootInfo) -> Result<&'static BootInfo, &'static str> {
    if info.is_null() {
        return Err("boot info is null");
    }
    let info = &*info;
    if info.magic != BOOTINFO_MAGIC {
        return Err("boot info magic mismatch");
    }
    if info.version != BOOTINFO_VERSION {
        return Err("boot info version mismatch");
    }
    Ok(info)
}
