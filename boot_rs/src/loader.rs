//! Port of the loading logic in `boot/main.c`: open the ESP root volume,
//! read+validate the kernel ELF, map its PT_LOAD segments to their exact
//! physical addresses, load the PSF1 font, find the GOP framebuffer, build
//! `BootInfo`, and hand back everything `main.rs` needs to exit boot
//! services and jump to the kernel. Kept as a single module (not split
//! further) because every step here shares the same `BootServices`/
//! `Handle` context and the C original wasn't split either — matching its
//! shape makes this easier to diff against while porting.

use core::ffi::c_void;
use core::ptr;

// EfiMemoryDescriptor comes from `uefi::*` below (bootinfo::BootInfoPayload
// reuses that same type — see bootinfo.rs).
use crate::bootinfo::{BootInfo, BootInfoPayload, Framebuffer, Psf1Font, Psf1Header, BOOTINFO_MAGIC, BOOTINFO_VERSION, PSF1_MAGIC};
use crate::elf::{Elf64Ehdr, Elf64Phdr, ELFCLASS64, ELFDATA2LSB, EM_X86_64, EV_CURRENT, PT_LOAD};
use crate::uefi::*;

pub enum LoadError {
    Uefi(&'static str),
}

pub type LResult<T> = Result<T, LoadError>;

fn err(msg: &'static str) -> LoadError {
    LoadError::Uefi(msg)
}

fn utf16_cstr(s: &str, buf: &mut [u16]) -> *const u16 {
    let mut i = 0;
    for unit in s.encode_utf16() {
        if i >= buf.len() - 1 {
            break;
        }
        buf[i] = unit;
        i += 1;
    }
    buf[i] = 0;
    buf.as_ptr()
}

unsafe fn open_root_volume(
    bs: &BootServices,
    image_handle: Handle,
) -> LResult<*mut FileProtocol> {
    let mut loaded_image: *mut c_void = ptr::null_mut();
    let status = (bs.handle_protocol)(
        image_handle,
        &LOADED_IMAGE_PROTOCOL_GUID,
        &mut loaded_image,
    );
    if is_error(status) || loaded_image.is_null() {
        return Err(err("HandleProtocol(LoadedImage) failed"));
    }
    let loaded_image = loaded_image as *mut LoadedImageProtocol;

    let mut fs: *mut c_void = ptr::null_mut();
    let status = (bs.handle_protocol)(
        (*loaded_image).device_handle,
        &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
        &mut fs,
    );
    if is_error(status) || fs.is_null() {
        return Err(err("HandleProtocol(SimpleFileSystem) failed"));
    }
    let fs = fs as *mut SimpleFileSystemProtocol;

    let mut root: *mut FileProtocol = ptr::null_mut();
    let status = ((*fs).open_volume)(fs, &mut root);
    if is_error(status) || root.is_null() {
        return Err(err("OpenVolume failed"));
    }
    Ok(root)
}

unsafe fn open_file(root: *mut FileProtocol, name: &str) -> LResult<*mut FileProtocol> {
    let mut name_buf = [0u16; 64];
    let name_ptr = utf16_cstr(name, &mut name_buf);
    let mut handle: *mut FileProtocol = ptr::null_mut();
    let status = ((*root).open)(root, &mut handle, name_ptr, EFI_FILE_MODE_READ, 0);
    if is_error(status) || handle.is_null() {
        return Err(err("file Open failed"));
    }
    Ok(handle)
}

/// Reads exactly `len` bytes at the file's current position into `buf`.
/// Matches the C original's single-Read-call-with-size-check behavior — not
/// a retry loop. If firmware ever does a short read here, this reports it
/// as an error rather than silently accepting partial data, same as before.
unsafe fn read_exact(file: *mut FileProtocol, buf: *mut c_void, len: usize) -> LResult<()> {
    let mut size = len;
    let status = ((*file).read)(file, &mut size, buf);
    if is_error(status) || size != len {
        return Err(err("short/failed file read"));
    }
    Ok(())
}

pub struct LoadedKernel {
    pub entry: u64,
    pub kernel_start: u64,
    pub kernel_end: u64,
}

unsafe fn load_kernel_elf(bs: &BootServices, root: *mut FileProtocol) -> LResult<LoadedKernel> {
    let file = open_file(root, "kernel.elf")?;

    let mut ehdr: Elf64Ehdr = core::mem::zeroed();
    read_exact(
        file,
        &mut ehdr as *mut _ as *mut c_void,
        core::mem::size_of::<Elf64Ehdr>(),
    )?;

    if ehdr.e_ident[0] != 0x7F || &ehdr.e_ident[1..4] != b"ELF" {
        return Err(err("not an ELF file"));
    }
    if ehdr.e_ident[4] != ELFCLASS64
        || ehdr.e_ident[5] != ELFDATA2LSB
        || ehdr.e_ident[6] != EV_CURRENT
        || ehdr.e_machine != EM_X86_64
        || ehdr.e_phentsize as usize != core::mem::size_of::<Elf64Phdr>()
        || ehdr.e_phnum == 0
    {
        return Err(err("unsupported ELF header"));
    }

    // Read program headers into a firmware-allocated pool buffer.
    let phdrs_size = ehdr.e_phnum as usize * ehdr.e_phentsize as usize;
    let mut phdrs_ptr: *mut c_void = ptr::null_mut();
    let status = (bs.allocate_pool)(EFI_LOADER_DATA, phdrs_size, &mut phdrs_ptr);
    if is_error(status) || phdrs_ptr.is_null() {
        return Err(err("AllocatePool(phdrs) failed"));
    }
    let mut pos = ehdr.e_phoff;
    let status = ((*file).set_position)(file, pos);
    if is_error(status) {
        return Err(err("SetPosition(phoff) failed"));
    }
    read_exact(file, phdrs_ptr, phdrs_size)?;
    let phdrs = phdrs_ptr as *const Elf64Phdr;

    let mut kernel_start: u64 = u64::MAX;
    let mut kernel_end: u64 = 0;

    for i in 0..ehdr.e_phnum as usize {
        let ph = &*phdrs.add(i);
        if ph.p_type != PT_LOAD {
            continue;
        }
        if ph.p_memsz == 0 || ph.p_filesz > ph.p_memsz {
            return Err(err("PT_LOAD segment size invalid"));
        }
        if ph.p_paddr == 0 || ph.p_paddr.checked_add(ph.p_memsz).is_none() {
            return Err(err("PT_LOAD segment range invalid"));
        }

        let pages = (ph.p_memsz + 0xFFF) / 0x1000;
        let mut paddr: PhysicalAddress = ph.p_paddr;
        let status = (bs.allocate_pages)(ALLOCATE_ADDRESS, EFI_LOADER_DATA, pages as usize, &mut paddr);
        if is_error(status) {
            return Err(err("AllocatePages(segment) failed"));
        }

        pos = ph.p_offset;
        let status = ((*file).set_position)(file, pos);
        if is_error(status) {
            return Err(err("SetPosition(segment) failed"));
        }
        read_exact(file, paddr as *mut c_void, ph.p_filesz as usize)?;

        if ph.p_memsz > ph.p_filesz {
            let bss = (paddr as *mut u8).add(ph.p_filesz as usize);
            core::ptr::write_bytes(bss, 0, (ph.p_memsz - ph.p_filesz) as usize);
        }

        if ph.p_paddr < kernel_start {
            kernel_start = ph.p_paddr;
        }
        if ph.p_paddr + ph.p_memsz > kernel_end {
            kernel_end = ph.p_paddr + ph.p_memsz;
        }
    }

    // e_entry is a higher-half VIRTUAL address (the kernel is linked at
    // KERNEL_VIRTUAL_BASE + physical offset — see kernel_rs/linker.ld);
    // kernel_start/kernel_end are physical (from p_paddr, where the bytes
    // actually landed). Compare against the virtual range, not the
    // physical one, or every entry point fails this check by construction.
    let vaddr_start = kernel_start + KERNEL_VIRTUAL_BASE;
    let vaddr_end = kernel_end + KERNEL_VIRTUAL_BASE;
    if ehdr.e_entry < vaddr_start || ehdr.e_entry >= vaddr_end {
        return Err(err("entry point outside loaded kernel"));
    }

    let _ = pos;
    Ok(LoadedKernel {
        entry: ehdr.e_entry,
        kernel_start,
        kernel_end,
    })
}

unsafe fn load_psf1_font(bs: &BootServices, root: *mut FileProtocol) -> LResult<*mut Psf1Font> {
    let file = open_file(root, "font.psf")?;

    let mut header_ptr: *mut c_void = ptr::null_mut();
    let hdr_size = core::mem::size_of::<Psf1Header>();
    let status = (bs.allocate_pool)(EFI_LOADER_DATA, hdr_size, &mut header_ptr);
    if is_error(status) || header_ptr.is_null() {
        return Err(err("AllocatePool(font header) failed"));
    }
    read_exact(file, header_ptr, hdr_size)?;
    let header = header_ptr as *mut Psf1Header;
    if (*header).magic != PSF1_MAGIC {
        return Err(err("font magic invalid"));
    }

    let mut glyph_size = (*header).chars_size as usize * 256;
    if (*header).mode == 1 {
        glyph_size = (*header).chars_size as usize * 512;
    }

    let status = ((*file).set_position)(file, hdr_size as u64);
    if is_error(status) {
        return Err(err("SetPosition(glyphs) failed"));
    }
    let mut glyph_ptr: *mut c_void = ptr::null_mut();
    let status = (bs.allocate_pool)(EFI_LOADER_DATA, glyph_size, &mut glyph_ptr);
    if is_error(status) || glyph_ptr.is_null() {
        return Err(err("AllocatePool(font glyphs) failed"));
    }
    read_exact(file, glyph_ptr, glyph_size)?;

    let mut font_ptr: *mut c_void = ptr::null_mut();
    let status = (bs.allocate_pool)(
        EFI_LOADER_DATA,
        core::mem::size_of::<Psf1Font>(),
        &mut font_ptr,
    );
    if is_error(status) || font_ptr.is_null() {
        return Err(err("AllocatePool(font obj) failed"));
    }
    let font = font_ptr as *mut Psf1Font;
    (*font).header = header;
    (*font).glyph_buffer = glyph_ptr;
    Ok(font)
}

unsafe fn find_gop_framebuffer(bs: &BootServices) -> LResult<*mut Framebuffer> {
    let mut gop_ptr: *mut c_void = ptr::null_mut();
    let status = (bs.locate_protocol)(&GRAPHICS_OUTPUT_PROTOCOL_GUID, ptr::null_mut(), &mut gop_ptr);
    if is_error(status) || gop_ptr.is_null() {
        return Err(err("LocateProtocol(GOP) failed"));
    }
    let gop = gop_ptr as *mut GraphicsOutputProtocol;
    let mode = &*(*gop).mode;
    let info = &*mode.info;

    let mut fb_ptr: *mut c_void = ptr::null_mut();
    let status = (bs.allocate_pool)(
        EFI_LOADER_DATA,
        core::mem::size_of::<Framebuffer>(),
        &mut fb_ptr,
    );
    if is_error(status) || fb_ptr.is_null() {
        return Err(err("AllocatePool(framebuffer) failed"));
    }
    let fb = fb_ptr as *mut Framebuffer;
    (*fb).base_address = mode.frame_buffer_base as *mut c_void;
    (*fb).buffer_size = mode.frame_buffer_size as u64;
    (*fb).width = info.horizontal_resolution;
    (*fb).height = info.vertical_resolution;
    (*fb).pixels_per_scan_line = info.pixels_per_scan_line;
    Ok(fb)
}

// Must match kernel_rs/src/vmm.rs's KERNEL_VIRTUAL_BASE and
// kernel_rs/linker.ld's KERNEL_VIRTUAL_BASE exactly — see those files'
// comments on this three-way constant.
const KERNEL_VIRTUAL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

// Bootstrap page-table scratch pool size. 3072 pages = 12MB: generous for a
// QEMU-scale identity map at 2MB granularity (a few thousand 2MB regions at
// most, each needing at most one new PD/PDPT/PML4 entry) plus the small
// higher-half kernel range at 4K granularity. bootstrap_paging::TablePool
// halts loudly on serial if this is ever exhausted rather than silently
// corrupting memory — see its doc comment.
const BOOTSTRAP_POOL_PAGES: usize = 3072;

pub struct PreparedBoot {
    pub boot_info: *mut BootInfo,
    pub kernel_entry: u64,
    pub map_key: usize,
}

/// Loads everything, builds `BootInfo`, gets the memory map, and calls
/// ExitBootServices — all in this one function, because nothing may
/// allocate between capturing the map key and the successful
/// ExitBootServices call or the key goes stale. `main.rs` only jumps to the
/// kernel after this returns; it does no further UEFI calls of its own.
pub unsafe fn prepare_boot(
    system_table: &SystemTable,
    image_handle: Handle,
) -> LResult<PreparedBoot> {
    let bs = &*system_table.boot_services;
    let root = open_root_volume(bs, image_handle)?;

    let kernel = load_kernel_elf(bs, root)?;
    let font = load_psf1_font(bs, root)?;
    let framebuffer = find_gop_framebuffer(bs)?;

    let mut boot_info_ptr: *mut c_void = ptr::null_mut();
    let status = (bs.allocate_pool)(
        EFI_LOADER_DATA,
        core::mem::size_of::<BootInfo>(),
        &mut boot_info_ptr,
    );
    if is_error(status) || boot_info_ptr.is_null() {
        return Err(err("AllocatePool(BootInfo) failed"));
    }
    let boot_info = boot_info_ptr as *mut BootInfo;
    (*boot_info).magic = BOOTINFO_MAGIC;
    (*boot_info).version = BOOTINFO_VERSION;
    (*boot_info).size = core::mem::size_of::<BootInfo>() as u32;
    (*boot_info).payload = BootInfoPayload {
        framebuffer,
        font,
        memory_map: ptr::null_mut(),
        memory_map_size: 0,
        memory_map_descriptor_size: 0,
        memory_map_descriptor_version: 0,
        rsdp: ptr::null_mut(),
        kernel_physical_start: kernel.kernel_start,
        kernel_physical_end: kernel.kernel_end,
        kernel_virtual_base: 0,
    };

    // Bootstrap page-table scratch pool: MUST be allocated while boot
    // services are still live (nothing can allocate after
    // ExitBootServices). The tables themselves are built after EBS
    // succeeds, using this pre-reserved memory as a bump allocator.
    let mut pool_phys: PhysicalAddress = 0;
    let status = (bs.allocate_pages)(
        ALLOCATE_ANY_PAGES,
        EFI_LOADER_DATA,
        BOOTSTRAP_POOL_PAGES,
        &mut pool_phys,
    );
    if is_error(status) {
        return Err(err("AllocatePages(bootstrap page-table pool) failed"));
    }

    // Memory map + ExitBootServices retry loop, same shape as boot/main.c:
    // GetMemoryMap can change size out from under a single fixed buffer
    // (AllocatePool itself can grow the map), so this retries with a fresh
    // buffer each time ExitBootServices reports EFI_INVALID_PARAMETER
    // rather than assuming one GetMemoryMap call is ever final.
    let mut map_size: usize = 0;
    let mut map_key: usize = 0;
    let mut descriptor_size: usize = 0;
    let mut descriptor_version: u32 = 0;

    let status = (bs.get_memory_map)(
        &mut map_size,
        ptr::null_mut(),
        &mut map_key,
        &mut descriptor_size,
        &mut descriptor_version,
    );
    // EFI_BUFFER_TOO_SMALL = 5 | EFI_ERROR_BIT
    if status != (5 | EFI_ERROR_BIT) {
        return Err(err("GetMemoryMap(size probe) unexpected status"));
    }
    map_size += 2 * descriptor_size;

    loop {
        let mut map_ptr: *mut c_void = ptr::null_mut();
        let status = (bs.allocate_pool)(EFI_LOADER_DATA, map_size, &mut map_ptr);
        if is_error(status) || map_ptr.is_null() {
            return Err(err("AllocatePool(memory map) failed"));
        }

        let mut this_map_size = map_size;
        let status = (bs.get_memory_map)(
            &mut this_map_size,
            map_ptr as *mut EfiMemoryDescriptor,
            &mut map_key,
            &mut descriptor_size,
            &mut descriptor_version,
        );
        if is_error(status) {
            return Err(err("GetMemoryMap failed"));
        }

        (*boot_info).payload.memory_map = map_ptr as *mut EfiMemoryDescriptor;
        (*boot_info).payload.memory_map_size = this_map_size as u64;
        (*boot_info).payload.memory_map_descriptor_size = descriptor_size as u64;
        (*boot_info).payload.memory_map_descriptor_version = descriptor_version;

        let status = (bs.exit_boot_services)(image_handle, map_key);
        if !is_error(status) {
            // Boot services are gone now — no more UEFI calls of any kind
            // from here on. Build the bootstrap tables using the pool
            // reserved earlier and the final memory map just captured
            // above, then switch CR3 before returning to main.rs for the
            // jump. This is real hardware manipulation, not a firmware
            // call, so it's fine to do post-EBS.
            let mut pool = crate::bootstrap_paging::TablePool::new(pool_phys, BOOTSTRAP_POOL_PAGES as u64);
            let pml4_phys = crate::bootstrap_paging::build(
                &mut pool,
                (*boot_info).payload.memory_map,
                (*boot_info).payload.memory_map_size,
                (*boot_info).payload.memory_map_descriptor_size,
                kernel.kernel_start,
                kernel.kernel_end,
                kernel.kernel_start + KERNEL_VIRTUAL_BASE,
            );
            core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nostack, preserves_flags));

            return Ok(PreparedBoot {
                boot_info,
                kernel_entry: kernel.entry,
                map_key,
            });
        }
        // EFI_INVALID_PARAMETER = 2 | EFI_ERROR_BIT: map changed, retry.
        if status != (2 | EFI_ERROR_BIT) {
            return Err(err("ExitBootServices failed"));
        }
        map_size += 2 * descriptor_size;
    }
}
