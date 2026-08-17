#pragma once
#include <stdint.h>

#define BOOTINFO_MAGIC 0x41474F53424F4F54ULL /* AGOSBOOT */
#define BOOTINFO_VERSION 1
#define PSF1_MAGIC 0x0436

typedef struct {
    uint16_t magic;
    uint8_t mode;
    uint8_t chars_size;
} PSF1_Header;

typedef struct {
    PSF1_Header* header;
    void* glyphBuffer;
} PSF1_Font;

typedef struct {
    void* BaseAddress;
    uint64_t BufferSize;
    uint32_t Width;
    uint32_t Height;
    uint32_t PixelsPerScanLine;
} Framebuffer;

// Memory Descriptor format from UEFI spec
typedef struct {
    uint32_t Type;
    uint32_t Pad;
    uint64_t PhysicalStart;
    uint64_t VirtualStart;
    uint64_t NumberOfPages;
    uint64_t Attribute;
} EFI_MEMORY_DESCRIPTOR_STRUCT;

typedef struct {
    Framebuffer* framebuffer;
    PSF1_Font* font;
    EFI_MEMORY_DESCRIPTOR_STRUCT* memory_map;
    uint64_t memory_map_size;
    uint64_t memory_map_descriptor_size;
    uint32_t memory_map_descriptor_version;
    void* rsdp;
    uint64_t kernel_physical_start;
    uint64_t kernel_physical_end;
    uint64_t kernel_virtual_base;
} BootInfoPayload;

typedef struct {
    uint64_t magic;
    uint32_t version;
    uint32_t size;
    BootInfoPayload payload;
} BootInfo;
