#include <efi.h>
#include <efilib.h>
#include "elf.h"
// Bootinfo includes stdint.h which UEFI doesn't provide by default, but our compiler does
#include <stdint.h>
#include "../include/bootinfo.h"

#define EI_CLASS 4
#define EI_DATA 5
#define EI_VERSION 6
#define ELFCLASS64 2
#define ELFDATA2LSB 1
#define EM_X86_64 62
#define EV_CURRENT 1

#define COM1 0x3F8

static inline void boot_outb(uint16_t port, uint8_t value) {
    __asm__ __volatile__("outb %0, %1" : : "a"(value), "Nd"(port));
}

static inline uint8_t boot_inb(uint16_t port) {
    uint8_t ret;
    __asm__ __volatile__("inb %1, %0" : "=a"(ret) : "Nd"(port));
    return ret;
}

static void boot_serial_init(void) {
    boot_outb(COM1 + 1, 0x00);
    boot_outb(COM1 + 3, 0x80);
    boot_outb(COM1 + 0, 0x03);
    boot_outb(COM1 + 1, 0x00);
    boot_outb(COM1 + 3, 0x03);
    boot_outb(COM1 + 2, 0xC7);
    boot_outb(COM1 + 4, 0x0B);
}

static void boot_serial_write_char(char c) {
    while ((boot_inb(COM1 + 5) & 0x20) == 0) {}
    boot_outb(COM1, (uint8_t)c);
}

static void boot_serial_write(const char* str) {
    while (*str) {
        if (*str == '\n') boot_serial_write_char('\r');
        boot_serial_write_char(*str++);
    }
}

static void halt_with_error(const char* message) {
    boot_serial_write(message);
    boot_serial_write("\n");
    while(1) { __asm__ __volatile__("hlt"); }
}

// Helper to open a file from the root directory safely
EFI_FILE* LoadFile(EFI_FILE* Directory, CHAR16* Path, EFI_HANDLE ImageHandle, EFI_SYSTEM_TABLE* SystemTable) {
    EFI_FILE* LoadedFile;
    EFI_STATUS s;

    EFI_GUID loadedImageProtocolGuid = EFI_LOADED_IMAGE_PROTOCOL_GUID;
    EFI_LOADED_IMAGE_PROTOCOL* LoadedImage;
    s = uefi_call_wrapper(SystemTable->BootServices->HandleProtocol, 3, ImageHandle, &loadedImageProtocolGuid, (void**)&LoadedImage);
    if (EFI_ERROR(s) || LoadedImage == NULL) {
        Print(L"[Error] Could not get LoadedImageProtocol (Status: %r)\n", s);
        return NULL;
    }

    EFI_GUID simpleFileSystemProtocolGuid = EFI_SIMPLE_FILE_SYSTEM_PROTOCOL_GUID;
    EFI_SIMPLE_FILE_SYSTEM_PROTOCOL* FileSystem;
    s = uefi_call_wrapper(SystemTable->BootServices->HandleProtocol, 3, LoadedImage->DeviceHandle, &simpleFileSystemProtocolGuid, (void**)&FileSystem);
    if (EFI_ERROR(s) || FileSystem == NULL) {
        Print(L"[Error] Could not get SimpleFileSystemProtocol (Status: %r)\n", s);
        return NULL;
    }

    if (Directory == NULL) {
        s = uefi_call_wrapper(FileSystem->OpenVolume, 2, FileSystem, &Directory);
        if (EFI_ERROR(s) || Directory == NULL) {
            Print(L"[Error] Could not open root volume\n");
            return NULL;
        }
    }

    s = uefi_call_wrapper(Directory->Open, 5, Directory, &LoadedFile, Path, EFI_FILE_MODE_READ, EFI_FILE_READ_ONLY);
    if (EFI_ERROR(s)) {
        Print(L"[Error] Could not open file: %s (Status: %r)\n", Path, s);
        return NULL;
    }
    
    return LoadedFile;
}

PSF1_Font* LoadPSF1Font(EFI_FILE* Directory, CHAR16* Path, EFI_HANDLE ImageHandle, EFI_SYSTEM_TABLE* SystemTable) {
    EFI_FILE* font = LoadFile(Directory, Path, ImageHandle, SystemTable);
    if (font == NULL) return NULL;

    PSF1_Header* fontHeader;
    EFI_STATUS s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, sizeof(PSF1_Header), (void**)&fontHeader);
    if (EFI_ERROR(s) || fontHeader == NULL) {
        Print(L"[Error] Could not allocate font header\n");
        return NULL;
    }
    UINTN size = sizeof(PSF1_Header);
    s = uefi_call_wrapper(font->Read, 3, font, &size, fontHeader);
    if (EFI_ERROR(s) || size != sizeof(PSF1_Header)) {
        Print(L"[Error] Could not read complete PSF1 header\n");
        return NULL;
    }

    if (fontHeader->magic != PSF1_MAGIC) {
        Print(L"[Error] Font magic invalid!\n");
        return NULL;
    }

    UINTN glyphBufferSize = fontHeader->chars_size * 256;
    if (fontHeader->mode == 1) { // 512 glyph mode
        glyphBufferSize = fontHeader->chars_size * 512;
    }

    void* glyphBuffer;
    s = uefi_call_wrapper(font->SetPosition, 2, font, sizeof(PSF1_Header));
    if (EFI_ERROR(s)) return NULL;
    s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, glyphBufferSize, (void**)&glyphBuffer);
    if (EFI_ERROR(s) || glyphBuffer == NULL) return NULL;
    UINTN expectedGlyphBufferSize = glyphBufferSize;
    s = uefi_call_wrapper(font->Read, 3, font, &glyphBufferSize, glyphBuffer);
    if (EFI_ERROR(s) || glyphBufferSize != expectedGlyphBufferSize) {
        Print(L"[Error] Could not read complete PSF1 glyph buffer\n");
        return NULL;
    }

    PSF1_Font* fontObj;
    s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, sizeof(PSF1_Font), (void**)&fontObj);
    if (EFI_ERROR(s) || fontObj == NULL) return NULL;
    fontObj->header = fontHeader;
    fontObj->glyphBuffer = glyphBuffer;

    return fontObj;
}

EFI_STATUS EFIAPI efi_main(EFI_HANDLE ImageHandle, EFI_SYSTEM_TABLE *SystemTable) {
    InitializeLib(ImageHandle, SystemTable);
    boot_serial_init();
    boot_serial_write("BOOT_START\n");
    
    uefi_call_wrapper(SystemTable->ConOut->ClearScreen, 1, SystemTable->ConOut);
    Print(L"Agentic OS Bootloader loading...\n");

    // Load the kernel file
    EFI_FILE* KernelFile = LoadFile(NULL, L"kernel.elf", ImageHandle, SystemTable);
    if (KernelFile == NULL) {
        Print(L"Could not open kernel.elf! Halted.\n");
        while(1) { __asm__ __volatile__("hlt"); }
        return EFI_LOAD_ERROR;
    }

    // Load the font
    PSF1_Font* newFont = LoadPSF1Font(NULL, L"font.psf", ImageHandle, SystemTable);
    if (newFont == NULL) {
        Print(L"Could not open font.psf! Halted.\n");
        while(1) { __asm__ __volatile__("hlt"); }
        return EFI_LOAD_ERROR;
    }
    Print(L"Font loaded successfully.\n");

    // Read the ELF Header
    Elf64_Ehdr header;
    UINTN size = sizeof(header);
    EFI_STATUS s = uefi_call_wrapper(KernelFile->Read, 3, KernelFile, &size, &header);
    if (EFI_ERROR(s) || size != sizeof(header)) {
        Print(L"Failed to read ELF header.\n");
        halt_with_error("BOOT_ERROR_ELF_HEADER_READ");
    }

    // Verify it's an ELF file
    if (header.e_ident[0] != 0x7F || header.e_ident[1] != 'E' || 
        header.e_ident[2] != 'L' || header.e_ident[3] != 'F') {
        Print(L"Kernel is not a valid ELF file.\n");
        halt_with_error("BOOT_ERROR_ELF_MAGIC");
    }
    if (header.e_ident[EI_CLASS] != ELFCLASS64 ||
        header.e_ident[EI_DATA] != ELFDATA2LSB ||
        header.e_ident[EI_VERSION] != EV_CURRENT ||
        header.e_machine != EM_X86_64 ||
        header.e_phentsize != sizeof(Elf64_Phdr) ||
        header.e_phnum == 0) {
        Print(L"Kernel ELF header is not supported.\n");
        halt_with_error("BOOT_ERROR_ELF_UNSUPPORTED");
    }
    Print(L"ELF format verified.\n");
    boot_serial_write("ELF_OK\n");

    // Read Program Headers and load segments
    Elf64_Phdr* phdrs;
    s = uefi_call_wrapper(KernelFile->SetPosition, 2, KernelFile, header.e_phoff);
    if (EFI_ERROR(s)) halt_with_error("BOOT_ERROR_PHDR_SEEK");
    UINTN phdrs_size = header.e_phnum * header.e_phentsize;
    UINTN expected_phdrs_size = phdrs_size;
    s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, phdrs_size, (void**)&phdrs);
    if (EFI_ERROR(s) || phdrs == NULL) halt_with_error("BOOT_ERROR_PHDR_ALLOC");
    s = uefi_call_wrapper(KernelFile->Read, 3, KernelFile, &phdrs_size, phdrs);
    if (EFI_ERROR(s) || phdrs_size != expected_phdrs_size) halt_with_error("BOOT_ERROR_PHDR_READ");

    uint64_t kernel_start = UINT64_MAX;
    uint64_t kernel_end = 0;

    for (int i = 0; i < header.e_phnum; i++) {
        if (phdrs[i].p_type == PT_LOAD) {
            if (phdrs[i].p_memsz == 0 || phdrs[i].p_filesz > phdrs[i].p_memsz) {
                halt_with_error("BOOT_ERROR_ELF_SEGMENT_SIZE");
            }
            if (phdrs[i].p_paddr == 0 || phdrs[i].p_paddr + phdrs[i].p_memsz < phdrs[i].p_paddr) {
                halt_with_error("BOOT_ERROR_ELF_SEGMENT_RANGE");
            }
            int pages = (phdrs[i].p_memsz + 0x0FFF) / 0x1000;
            EFI_PHYSICAL_ADDRESS paddr = phdrs[i].p_paddr;
            
            // Allocate the exact physical address requested by the ELF
            s = uefi_call_wrapper(SystemTable->BootServices->AllocatePages, 4, AllocateAddress, EfiLoaderData, pages, &paddr);
            if (EFI_ERROR(s)) {
                Print(L"Failed to allocate memory for segment %d (Status: %r)\n", i, s);
                while(1) { __asm__ __volatile__("hlt"); }
            }

            s = uefi_call_wrapper(KernelFile->SetPosition, 2, KernelFile, phdrs[i].p_offset);
            if (EFI_ERROR(s)) halt_with_error("BOOT_ERROR_SEGMENT_SEEK");
            UINTN read_size = phdrs[i].p_filesz;
            UINTN expected_read_size = read_size;
            s = uefi_call_wrapper(KernelFile->Read, 3, KernelFile, &read_size, (void*)paddr);
            if (EFI_ERROR(s) || read_size != expected_read_size) halt_with_error("BOOT_ERROR_SEGMENT_READ");
            
            // Zero out remaining memory in segment if memsz > filesz (BSS)
            if (phdrs[i].p_memsz > phdrs[i].p_filesz) {
                uint8_t* bss = (uint8_t*)paddr + phdrs[i].p_filesz;
                for(UINTN j = 0; j < phdrs[i].p_memsz - phdrs[i].p_filesz; j++) {
                    bss[j] = 0;
                }
            }

            if (phdrs[i].p_paddr < kernel_start) kernel_start = phdrs[i].p_paddr;
            if (phdrs[i].p_paddr + phdrs[i].p_memsz > kernel_end) {
                kernel_end = phdrs[i].p_paddr + phdrs[i].p_memsz;
            }
        }
    }
    if (header.e_entry < kernel_start || header.e_entry >= kernel_end) {
        halt_with_error("BOOT_ERROR_ENTRY_OUTSIDE_KERNEL");
    }
    Print(L"Kernel segments loaded into memory.\n");

    // Get Graphics Output Protocol (GOP) for the Framebuffer
    EFI_GRAPHICS_OUTPUT_PROTOCOL* gop;
    EFI_GUID gopGuid = EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID;
    s = uefi_call_wrapper(SystemTable->BootServices->LocateProtocol, 3, &gopGuid, NULL, (void**)&gop);
    if (EFI_ERROR(s) || gop == NULL) {
        Print(L"Failed to locate GOP (Status: %r)\n", s);
        while(1) { __asm__ __volatile__("hlt"); }
    }

    Framebuffer* fb;
    s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, sizeof(Framebuffer), (void**)&fb);
    if (EFI_ERROR(s) || fb == NULL) halt_with_error("BOOT_ERROR_FRAMEBUFFER_ALLOC");
    fb->BaseAddress = (void*)gop->Mode->FrameBufferBase;
    fb->BufferSize = gop->Mode->FrameBufferSize;
    fb->Width = gop->Mode->Info->HorizontalResolution;
    fb->Height = gop->Mode->Info->VerticalResolution;
    fb->PixelsPerScanLine = gop->Mode->Info->PixelsPerScanLine;

    BootInfo* bootInfo;
    s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, sizeof(BootInfo), (void**)&bootInfo);
    if (EFI_ERROR(s) || bootInfo == NULL) halt_with_error("BOOT_ERROR_BOOTINFO_ALLOC");
    bootInfo->magic = BOOTINFO_MAGIC;
    bootInfo->version = BOOTINFO_VERSION;
    bootInfo->size = sizeof(BootInfo);
    bootInfo->payload.framebuffer = fb;
    bootInfo->payload.font = newFont;
    bootInfo->payload.rsdp = NULL;
    bootInfo->payload.kernel_physical_start = kernel_start;
    bootInfo->payload.kernel_physical_end = kernel_end;
    bootInfo->payload.kernel_virtual_base = 0;

    Print(L"Retrieving Memory Map and Exiting Boot Services...\n");

    // Memory Map & ExitBootServices retry loop (production-grade standard)
    EFI_MEMORY_DESCRIPTOR* Map = NULL;
    UINTN MapSize = 0, MapKey = 0;
    UINTN DescriptorSize = 0;
    UINT32 DescriptorVersion = 0;
    
    // First call to get the size needed
    s = uefi_call_wrapper(SystemTable->BootServices->GetMemoryMap, 5, &MapSize, Map, &MapKey, &DescriptorSize, &DescriptorVersion);
    if (s != EFI_BUFFER_TOO_SMALL) halt_with_error("BOOT_ERROR_MEMORY_MAP_SIZE");
    
    // Add extra space because AllocatePool might expand the map
    MapSize += 2 * DescriptorSize;
    s = uefi_call_wrapper(SystemTable->BootServices->AllocatePool, 3, EfiLoaderData, MapSize, (void**)&Map);
    if (EFI_ERROR(s) || Map == NULL) halt_with_error("BOOT_ERROR_MEMORY_MAP_ALLOC");

    while (1) {
        // Get the actual map
        s = uefi_call_wrapper(SystemTable->BootServices->GetMemoryMap, 5, &MapSize, Map, &MapKey, &DescriptorSize, &DescriptorVersion);
        if (EFI_ERROR(s)) {
            if (s == EFI_BUFFER_TOO_SMALL) {
                halt_with_error("BOOT_ERROR_MEMORY_MAP_TOO_SMALL_AFTER_ALLOC");
            }
            halt_with_error("BOOT_ERROR_MEMORY_MAP_READ");
        }
        
        // Populate BootInfo with memory map
        bootInfo->payload.memory_map = (EFI_MEMORY_DESCRIPTOR_STRUCT*)Map;
        bootInfo->payload.memory_map_size = MapSize;
        bootInfo->payload.memory_map_descriptor_size = DescriptorSize;
        bootInfo->payload.memory_map_descriptor_version = DescriptorVersion;
        boot_serial_write("MEMORY_MAP_OK\n");

        // Try to exit boot services immediately
        s = uefi_call_wrapper(SystemTable->BootServices->ExitBootServices, 2, ImageHandle, MapKey);
        if (!EFI_ERROR(s)) {
            break; // Success! We own the hardware now.
        }
        if (s != EFI_INVALID_PARAMETER) halt_with_error("BOOT_ERROR_EXIT_BOOT_SERVICES");
    }
    boot_serial_write("EXIT_BOOT_SERVICES_OK\n");

    // Jump to the kernel!
    boot_serial_write("KERNEL_ENTER\n");
    void (*KernelStart)(BootInfo*) = ((__attribute__((sysv_abi)) void (*)(BootInfo*) ) header.e_entry);
    KernelStart(bootInfo);

    return EFI_SUCCESS;
}
