//! Minimal hand-written UEFI table bindings — only what this first slice
//! needs (`EFI_SYSTEM_TABLE` through `ConOut`). Written against the UEFI
//! Specification directly, not a helper crate, matching the discipline in
//! `kernel_rs/`. This grows incrementally as the bootloader port progresses
//! (BootServices, file protocols, GOP, memory map — see BOOT0_PROGRESS.md);
//! it deliberately does not model fields this slice doesn't touch yet.

#![allow(dead_code)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Status = usize;

pub const EFI_SUCCESS: Status = 0;

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct SimpleTextOutputProtocol {
    pub reset:
        unsafe extern "efiapi" fn(this: *mut SimpleTextOutputProtocol, extended: u8) -> Status,
    pub output_string:
        unsafe extern "efiapi" fn(this: *mut SimpleTextOutputProtocol, string: *const u16) -> Status,
    // ClearScreen, SetCursorPosition, etc. not modeled yet — not needed by
    // this slice. Add as the bootloader port needs them.
}

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *mut u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut c_void, // EFI_SIMPLE_TEXT_INPUT_PROTOCOL*, not modeled yet
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutputProtocol,
    // StdErr, RuntimeServices, BootServices, ConfigurationTable, etc. follow
    // in the real struct and are NOT modeled yet. Reading past `con_out`
    // through this struct definition would read garbage — do not extend
    // usage past what's declared here without adding the real fields first.
}

/// Writes an ASCII string through `ConOut`, converting to UTF-16 and
/// terminating with `\0` as `EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL.OutputString`
/// requires. Truncates past 127 chars rather than allocating (no heap yet).
pub fn print(con_out: *mut SimpleTextOutputProtocol, s: &str) {
    let mut buf = [0u16; 128];
    let mut i = 0;
    for unit in s.encode_utf16() {
        if i >= buf.len() - 1 {
            break;
        }
        buf[i] = unit;
        i += 1;
    }
    buf[i] = 0;
    unsafe {
        ((*con_out).output_string)(con_out, buf.as_ptr());
    }
}
