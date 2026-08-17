# M1 Prompt: Bootloader Hardening

## Prompt For Antigravity

Implement milestone `M1: Bootloader Hardening` for Agentic OS.

Inspect `boot/main.c`, `boot/elf.h`, `include/bootinfo.h`, `Makefile`, and existing serial evidence before editing.

Goal: make the UEFI boot path deterministic, validated, and diagnosable.

## Required Changes

- Validate every critical UEFI call in the bootloader:
  - `HandleProtocol`
  - `OpenVolume`
  - file `Open`
  - file `Read`
  - file `SetPosition`
  - `AllocatePool`
  - `AllocatePages`
  - `LocateProtocol`
  - `GetMemoryMap`
  - `ExitBootServices`
- Harden ELF validation:
  - magic bytes
  - `ELFCLASS64`
  - little-endian
  - current ELF version
  - `EM_X86_64`
  - nonzero program header count
  - expected program header entry size
  - `PT_LOAD` segment ranges
  - `p_filesz <= p_memsz`
  - entry point lies inside a loaded segment
- Harden `font.psf` loading:
  - validate allocation
  - validate full header read
  - validate PSF1 magic
  - validate full glyph read
- Keep or introduce versioned `BootInfo`:
  - `magic`
  - `version`
  - `size`
  - framebuffer metadata
  - font metadata
  - memory map pointer and descriptor metadata
  - kernel physical start/end
  - future `rsdp` pointer field
- Emit serial checkpoints:
  - `BOOT_START`
  - `ELF_OK`
  - `MEMORY_MAP_OK`
  - `EXIT_BOOT_SERVICES_OK`
  - `KERNEL_ENTER`
- Avoid allocating inside the `ExitBootServices` retry loop except when explicitly handling the initial buffer sizing path.

## Non-Goals

- Do not implement ACPI parsing beyond passing an `rsdp` placeholder unless already simple and safe.
- Do not redesign kernel memory management here.
- Do not add user mode or filesystem.

## Verification

Run:

```sh
make clean all
make test-boot
```

Inspect `_evidence/latest/serial.log` and confirm all required bootloader checkpoints appear in order.

## Acceptance Criteria

- Valid image boots to kernel.
- Corrupt/unsupported ELF paths fail with serial-visible error strings.
- Short reads and failed allocations do not fall through into undefined behavior.
- `BootInfo` remains backward-explicit through `magic`, `version`, and `size`.
- Final report lists any UEFI calls that still cannot be fully validated and why.
