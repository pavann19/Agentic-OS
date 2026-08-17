# Agentic OS Architecture

## Layer Model

Agentic OS is structured as layered software, with strong separation between deterministic kernel work and agentic user-space behavior.

- Firmware and bootloader: UEFI application loads `kernel.elf`, validates boot assets, gathers framebuffer/font/memory metadata, exits boot services, then jumps to the kernel.
- Kernel core: logging, panic handling, GDT/IDT, PMM, VMM, heap, interrupts, timers, scheduler, syscalls, process isolation.
- Hardware layer: CPU features, APIC/IOAPIC, timers, framebuffer, keyboard, disk, filesystem, network and USB later.
- System services: init, service manager, device manager, logs, configuration, permissions, indexing.
- Agentic layer: agent runtime, policy enforcement, tool execution, memory/indexing, audit logs.
- UX layer: shell first, desktop later, agent-native workflows after the base OS is trustworthy.

## Boot Sequence

1. UEFI enters `BOOTX64.EFI`.
2. Bootloader initializes serial and emits `BOOT_START`.
3. Bootloader loads and validates `kernel.elf`.
4. Bootloader loads and validates `font.psf`.
5. Bootloader finds GOP framebuffer metadata.
6. Bootloader builds a versioned `BootInfo`.
7. Bootloader captures the UEFI memory map and emits `MEMORY_MAP_OK`.
8. Bootloader exits boot services and emits `EXIT_BOOT_SERVICES_OK`.
9. Bootloader jumps to the kernel entry and emits `KERNEL_ENTER`.
10. Kernel validates `BootInfo`, initializes serial logging, diagnostics, memory, paging, graphics, and interrupts.

## BootInfo Contract

`BootInfo` is versioned and starts with:

- `magic`: must equal `BOOTINFO_MAGIC`.
- `version`: must equal `BOOTINFO_VERSION`.
- `size`: must equal the structure size expected by the kernel.
- `payload`: contains framebuffer, font, memory map, RSDP placeholder, kernel physical range, and kernel virtual base.

The bootloader owns creating this structure. The kernel must validate it before using any pointer from it.

## Memory Policy

The current PMM still needs redesign. The target policy is:

- Page zero remains reserved and unmapped.
- Usable pages come only from firmware descriptors that are truly usable.
- Firmware holes, MMIO regions, kernel image pages, boot info, framebuffer, page bitmap, and page tables are reserved explicitly.
- Normal page allocation must not imply physical contiguity.
- Contiguous physical allocation must use a separate explicit API.

## Virtual Memory Policy

The target VMM layout is:

- Null guard region unmapped.
- Temporary low identity mapping only during early transition.
- Higher-half kernel mapping.
- Physical direct-map window.
- Dedicated MMIO window.
- Kernel heap window.
- User-space region below the kernel split.

The current implementation still identity maps broad physical memory and must be replaced before user mode.

## Interrupt Policy

Interrupt handlers should do the least possible work:

- Capture event state.
- Acknowledge the interrupt controller.
- Defer rendering, parsing, scheduling, and expensive work.

The current keyboard handler still renders directly through the framebuffer path and must be moved to an input queue in a later phase.

## User/Kernel Boundary

User-space support is not present yet. The target boundary is:

- User programs run in ring 3.
- Syscalls validate all user pointers.
- User page faults kill the offending process when possible.
- Kernel faults panic with serial evidence.
- Agentic services run as user-space processes, never in kernel mode.
