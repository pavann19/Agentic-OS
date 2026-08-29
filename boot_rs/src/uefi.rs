//! Hand-written UEFI table bindings, extended incrementally as the
//! bootloader port needs more of the spec. Field ORDER matters — these are
//! `#[repr(C)]` structs read via raw pointer against real firmware memory,
//! so every field before the one actually used must be present and
//! correctly typed/sized even when this code never touches it, or every
//! field after it silently reads at the wrong offset. Trailing fields this
//! code doesn't need are simply omitted (safe: we never read past what's
//! declared), never reordered or skipped mid-struct.

#![allow(dead_code)]

use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Status = usize;
pub type PhysicalAddress = u64;

pub const EFI_SUCCESS: Status = 0;
// EFI_ERROR bit (bit 63 on x86_64) is set on every failure status.
pub const EFI_ERROR_BIT: usize = 1 << 63;
pub fn is_error(status: Status) -> bool {
    (status & EFI_ERROR_BIT) != 0
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Guid(pub u32, pub u16, pub u16, pub [u8; 8]);

pub const LOADED_IMAGE_PROTOCOL_GUID: Guid = Guid(
    0x5B1B31A1,
    0x9562,
    0x11d2,
    [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const SIMPLE_FILE_SYSTEM_PROTOCOL_GUID: Guid = Guid(
    0x964E5B22,
    0x6459,
    0x11D2,
    [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
);
pub const GRAPHICS_OUTPUT_PROTOCOL_GUID: Guid = Guid(
    0x9042A9DE,
    0x23DC,
    0x4A38,
    [0x96, 0xFB, 0x7A, 0xDE, 0xD0, 0x80, 0x51, 0x6A],
);

#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

// ---- Console output ----

#[repr(C)]
pub struct SimpleTextOutputProtocol {
    pub reset:
        unsafe extern "efiapi" fn(this: *mut SimpleTextOutputProtocol, extended: u8) -> Status,
    pub output_string:
        unsafe extern "efiapi" fn(this: *mut SimpleTextOutputProtocol, string: *const u16) -> Status,
}

// ---- Memory map ----

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EfiMemoryDescriptor {
    pub ty: u32,
    pub pad: u32,
    pub physical_start: u64,
    pub virtual_start: u64,
    pub number_of_pages: u64,
    pub attribute: u64,
}

pub const ALLOCATE_ADDRESS: u32 = 2;
pub const EFI_LOADER_DATA: u32 = 2;
pub const EFI_CONVENTIONAL_MEMORY: u32 = 7;

// ---- Boot services ----
//
// Field order and count below matches the UEFI spec's EFI_BOOT_SERVICES
// table exactly, through CreateEventEx (the last field in the real table).
// Every function pointer this bootloader doesn't call is still declared
// (as an opaque `usize`) so offsets of the ones we DO call land correctly.

#[repr(C)]
pub struct BootServices {
    pub hdr: TableHeader,
    pub raise_tpl: usize,
    pub restore_tpl: usize,
    pub allocate_pages: unsafe extern "efiapi" fn(
        alloc_type: u32,
        memory_type: u32,
        pages: usize,
        memory: *mut PhysicalAddress,
    ) -> Status,
    pub free_pages: usize,
    pub get_memory_map: unsafe extern "efiapi" fn(
        memory_map_size: *mut usize,
        memory_map: *mut EfiMemoryDescriptor,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        descriptor_version: *mut u32,
    ) -> Status,
    pub allocate_pool: unsafe extern "efiapi" fn(
        pool_type: u32,
        size: usize,
        buffer: *mut *mut c_void,
    ) -> Status,
    pub free_pool: usize,
    pub create_event: usize,
    pub set_timer: usize,
    pub wait_for_event: usize,
    pub signal_event: usize,
    pub close_event: usize,
    pub check_event: usize,
    pub install_protocol_interface: usize,
    pub reinstall_protocol_interface: usize,
    pub uninstall_protocol_interface: usize,
    pub handle_protocol: unsafe extern "efiapi" fn(
        handle: Handle,
        protocol: *const Guid,
        interface: *mut *mut c_void,
    ) -> Status,
    pub reserved: usize,
    pub register_protocol_notify: usize,
    pub locate_handle: usize,
    pub locate_device_path: usize,
    pub install_configuration_table: usize,
    pub load_image: usize,
    pub start_image: usize,
    pub exit: usize,
    pub unload_image: usize,
    pub exit_boot_services:
        unsafe extern "efiapi" fn(image_handle: Handle, map_key: usize) -> Status,
    pub get_next_monotonic_count: usize,
    pub stall: usize,
    pub set_watchdog_timer: usize,
    pub connect_controller: usize,
    pub disconnect_controller: usize,
    pub open_protocol: usize,
    pub close_protocol: usize,
    pub open_protocol_information: usize,
    pub protocols_per_handle: usize,
    pub locate_handle_buffer: usize,
    pub locate_protocol: unsafe extern "efiapi" fn(
        protocol: *const Guid,
        registration: *mut c_void,
        interface: *mut *mut c_void,
    ) -> Status,
    pub install_multiple_protocol_interfaces: usize,
    pub uninstall_multiple_protocol_interfaces: usize,
    pub calculate_crc32: usize,
    pub copy_mem: usize,
    pub set_mem: usize,
    pub create_event_ex: usize,
}

// ---- System table (extended past ConOut to reach BootServices) ----

#[repr(C)]
pub struct SystemTable {
    pub hdr: TableHeader,
    pub firmware_vendor: *mut u16,
    pub firmware_revision: u32,
    pub console_in_handle: Handle,
    pub con_in: *mut c_void,
    pub console_out_handle: Handle,
    pub con_out: *mut SimpleTextOutputProtocol,
    pub standard_error_handle: Handle,
    pub std_err: *mut c_void,
    pub runtime_services: *mut c_void,
    pub boot_services: *mut BootServices,
    // ConfigurationTable/NumberOfTableEntries follow; not needed yet.
}

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

// ---- Loaded image protocol (through DeviceHandle only) ----

#[repr(C)]
pub struct LoadedImageProtocol {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    // FilePath and later fields not needed.
}

// ---- Simple file system + file protocol ----

#[repr(C)]
pub struct SimpleFileSystemProtocol {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(
        this: *mut SimpleFileSystemProtocol,
        root: *mut *mut FileProtocol,
    ) -> Status,
}

pub const EFI_FILE_MODE_READ: u64 = 0x1;

#[repr(C)]
pub struct FileProtocol {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        new_handle: *mut *mut FileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
    ) -> Status,
    pub close: unsafe extern "efiapi" fn(this: *mut FileProtocol) -> Status,
    pub delete: usize,
    pub read: unsafe extern "efiapi" fn(
        this: *mut FileProtocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
    pub write: usize,
    pub get_position: usize,
    pub set_position:
        unsafe extern "efiapi" fn(this: *mut FileProtocol, position: u64) -> Status,
    // GetInfo/SetInfo/Flush and revision-2 fields not needed.
}

// ---- Graphics output protocol ----

#[repr(C)]
pub struct GraphicsOutputProtocol {
    pub query_mode: usize,
    pub set_mode: usize,
    pub blt: usize,
    pub mode: *mut GraphicsOutputProtocolMode,
}

#[repr(C)]
pub struct GraphicsOutputProtocolMode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *mut GraphicsOutputModeInformation,
    pub size_of_info: usize,
    pub frame_buffer_base: PhysicalAddress,
    pub frame_buffer_size: usize,
}

#[repr(C)]
pub struct GraphicsOutputModeInformation {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub pixel_information: [u32; 4],
    pub pixels_per_scan_line: u32,
}
