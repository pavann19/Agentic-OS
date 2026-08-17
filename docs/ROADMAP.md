# Agentic OS Roadmap

## M0: Discipline And Boot Evidence

Status: in progress.

Acceptance criteria:

- `README.md` explains target architecture and workflow.
- `docs/ARCHITECTURE.md` records the system boundaries.
- Docker runs a real build command.
- `make test-boot` captures serial output under `_evidence/latest/`.
- Serial log includes deterministic boot checkpoints.

## M1: Bootloader Hardening

Status: partially implemented.

Acceptance criteria:

- Bootloader validates UEFI call results.
- ELF validation rejects unsupported or corrupted kernels.
- Font loading rejects short or corrupt reads.
- Bootloader passes versioned `BootInfo`.
- Serial checkpoints include `BOOT_START`, `ELF_OK`, `MEMORY_MAP_OK`, `EXIT_BOOT_SERVICES_OK`, `KERNEL_ENTER`.

## M2: Kernel Diagnostics

Status: partially implemented.

Acceptance criteria:

- Kernel serial logging is initialized before core subsystems.
- Panic disables interrupts, logs reason, and halts.
- Exceptions log fault identity and key registers.
- Framebuffer output remains secondary to serial logs.

## M3: Physical Memory Manager Redesign

Status: not complete.

Acceptance criteria:

- PMM stops using highest physical address as usable memory.
- Reserved ranges are tracked explicitly.
- Page zero is reserved and remains unmapped.
- Normal allocation does not imply contiguity.
- PMM stats distinguish physical span, usable RAM, free pages, and reserved pages.

## M4: Virtual Memory Manager

Status: not complete.

Acceptance criteria:

- Kernel uses a defined virtual layout.
- Null page is unmapped and faults.
- Only required regions are mapped.
- Page permissions are enforced.
- Map/unmap APIs invalidate TLB entries correctly.

## M5: Heap And Runtime

Status: not started.

Acceptance criteria:

- Kernel heap supports `kmalloc`, `kcalloc`, `kfree`, and alignment.
- Core memory/string primitives exist.
- Graphics back buffer uses contiguous virtual memory, not assumed contiguous physical pages.

## M6: Interrupts And Timers

Status: not started.

Acceptance criteria:

- CPU exceptions `0-31` are covered.
- TSS/IST protects double fault path.
- Timer interrupts fire reliably.
- Keyboard IRQ defers work through an input queue.

## M7-M11: Product Base

Status: future.

Acceptance criteria:

- Scheduler runs multiple kernel threads.
- User mode executes `/bin/init`.
- Syscall ABI supports minimal process I/O.
- VFS reads files from disk.
- Shell can inspect memory, processes, logs, and files.
- Automated QEMU tests validate build, boot, faults, and init.
