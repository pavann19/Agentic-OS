# Agentic OS

Agentic OS is an independent `x86_64` UEFI operating system prototype. The current repository contains the kernel seed: a UEFI bootloader, framebuffer output, PSF font rendering, basic memory management, paging, interrupts, keyboard input, and a diagnostic desktop.

The product direction is a real OS that boots directly on hardware or QEMU without depending on Windows, Linux, WSL, or another host subsystem at runtime. Build tools can run inside Docker, but the generated `os-image.img` is the operating system artifact.

## Current Target

- Architecture: `x86_64`
- Firmware: UEFI with OVMF during development
- Kernel language: freestanding C
- First runtime target: QEMU + OVMF
- Bare-metal target: after automated QEMU boot evidence is reliable

## Developer Workflow

Build the image:

```powershell
docker compose run --rm os-build make clean all
```

Run a headless boot smoke test:

```powershell
docker compose run --rm os-build make test-boot
```

Run host-side tests:

```powershell
docker compose run --rm os-build make test-host
```

Run interactively from inside the build container:

```sh
make run-qemu
```

## Evidence Policy

Every implementation task must produce evidence under `_evidence/latest/` when tests are run. The minimum useful evidence is:

- `serial.log`: QEMU serial output with boot checkpoints.
- `host-tests.log`: host-side test output when available.
- A changed-file summary in the agent final report.
- Known failures or skipped checks in the agent final report.

The current boot smoke test requires these serial checkpoints:

- `KERNEL_ENTER`
- `PMM_INIT_DONE`

## Product Boundary

The kernel must stay deterministic and small. Agentic behavior belongs in user-space services after process isolation, filesystem, IPC, and audit logging exist.

Near-term work should prioritize boot reliability, diagnostics, memory correctness, and automated testing before GUI or AI features.
