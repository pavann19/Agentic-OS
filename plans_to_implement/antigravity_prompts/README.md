# Antigravity Implementation Prompt Pack

This folder contains copy-paste ready prompts for Antigravity 3.1 Pro to implement Agentic OS in bounded stages.

Use these prompts in order. Do not skip ahead unless the previous stage has passing evidence and the orchestrator has accepted it.

## How To Use

1. Give Antigravity `00_GLOBAL_RULES.md` first in every session.
2. Give exactly one stage prompt at a time.
3. Require Antigravity to write evidence under `_evidence/latest/`.
4. Review the diff before reading the agent's summary.
5. Run the verification commands yourself before accepting the stage.

## Prompt Order

- `00_GLOBAL_RULES.md`: rules every stage must follow.
- `01_M0_PROJECT_DISCIPLINE_AND_EVIDENCE.md`: docs, workflow, evidence baseline.
- `02_M1_BOOTLOADER_HARDENING.md`: UEFI loader validation and boot metadata.
- `03_M2_KERNEL_DIAGNOSTICS.md`: serial logging, panic, fault visibility.
- `04_M3_PHYSICAL_MEMORY_MANAGER.md`: trustworthy physical page ownership.
- `05_M4_VIRTUAL_MEMORY_MANAGER.md`: real virtual memory layout.
- `06_M5_HEAP_AND_RUNTIME.md`: kernel heap and core runtime primitives.
- `07_M6_INTERRUPTS_EXCEPTIONS_TIMERS.md`: full exception coverage, TSS/IST, timer/input.
- `08_M7_SCHEDULER_THREADS.md`: kernel threads and scheduling.
- `09_M8_USER_MODE_SYSCALLS.md`: ring 3, syscalls, user ELF.
- `10_M9_STORAGE_FILESYSTEM_INIT.md`: block layer, VFS, FAT32, `/bin/init`.
- `11_M10_MINIMAL_UX_AGENTIC_PREP.md`: shell, diagnostics UX, agent service boundaries.
- `12_M11_AUTOMATED_TESTING_RELEASE.md`: complete regression and release evidence.
- `13_ORCHESTRATOR_ACCEPTANCE_CHECKLIST.md`: manual checks for the orchestrator.

## Current Baseline

At the time this prompt pack was written, the project already has:

- UEFI bootloader and `kernel.elf` image creation.
- Versioned `BootInfo`.
- Serial boot checkpoints.
- Kernel serial logging and panic primitives.
- QEMU smoke target through `make test-boot`.
- Evidence folder convention.

Future agents must inspect the current code before editing because the baseline can drift.
