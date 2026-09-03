# Agentic OS

A from-scratch `x86_64` UEFI operating system, written in Rust, built around capability-based security and hardware-contained autonomous driver synthesis rather than retrofitted onto an existing kernel.

Agentic OS does not run on, inside, or on top of Linux, Windows, or any other host kernel at runtime. It boots directly on real UEFI firmware or QEMU. The syscall ABI is capability-based from the first syscall — no POSIX file descriptors, no ambient authority, no uid/gid permission model.

## Why

Most "AI agent operating systems" are governance layers running as a process on a conventional OS: they can decide that an action is disallowed, but they have no way to make a *device* physically unable to do something, because the decision and the enforcement mechanism live in different places. Agentic OS starts from the opposite end: capabilities are enforced by the kernel's own syscall ABI and by real IOMMU hardware, so a driver process — including one an autonomous agent synthesized at runtime — is contained by silicon, not by policy it could theoretically be talked out of.

## What's real today

Every claim below is backed by a reproducible QEMU boot log or `cargo test` run, not aspiration:

- **Boot chain** — real UEFI bootloader (`boot_rs`), ELF64 kernel loader, higher-half kernel with permission-correct page tables (NX, read-only `.text`, per-page flags), a real physical direct-map window.
- **Core kernel** — GDT/TSS with IST-backed double-fault handling, all 32 CPU exception vectors, physical/virtual memory management, a heap allocator, Local APIC timer, deferred (non-blocking) interrupt handling.
- **Capabilities** — a capability-based syscall ABI: per-process capability tables, grant/attenuate/revoke, synchronous IPC, and a kernel-level audit log where capability invocation and audit-record emission are the same code path — a capability cannot be exercised without producing a record.
- **Drivers, in user space** — PCIe enumeration, a device manager with crash/restart, and real drivers built from spec + live PCI configuration space: `virtio-blk`, `virtio-net`, AHCI (SATA), NVMe, and Intel e1000. Every driver runs in ring 3 as a real ELF process, mediated by capabilities, never in the kernel.
- **IOMMU containment** — real Intel VT-d: DMAR table parsing, root/context tables, per-device DMA domains, real-time fault detection. A driver process can only DMA into memory its own domain explicitly maps; everything else is unreachable by construction, not by convention.
- **Filesystem** — a real ext2 implementation, an object store with capability-scoped naming (a process without a capability to an object cannot even discover it exists), audit log persistence with rotation.
- **Agent runtime substrate** — structured, typed system introspection (an agent enumerates processes, devices, and storage as machine-legible objects, never by scraping text), a typed tool/intent surface, and a policy engine enforced at the kernel's own capability-grant boundary.
- **Driver synthesis loop** — a snapshot/rollback test harness that lets an agent-synthesized driver run against disposable QEMU state, captures real induced failures (kernel log, IOMMU violations, timeouts) as structured feedback, and retries within a bounded budget.
- **Shell** — a capability-aware text shell over serial, with a natural-language intent path that resolves through the same typed tool surface a human-typed command does.
- **SMP** — real ACPI MADT CPU enumeration and a from-scratch AP bring-up trampoline (real mode → protected mode → long mode), bringing every firmware-reported core online.
- **Reproducible builds** — the UEFI bootloader build is byte-for-byte reproducible from source, independently verified via hash comparison across clean rebuilds.

## Non-goals

- Binary compatibility with Linux, Windows, or POSIX applications, ever.
- A conventional desktop environment ported in from outside the capability model.
- Multi-architecture support before the roadmap's current `x86_64` scope is complete.
- Any model inference or agent reasoning inside the kernel itself.

## Building and testing

Requires: Rust nightly with the `x86_64-unknown-none` and `x86_64-unknown-uefi` targets, QEMU with OVMF firmware, and Windows + PowerShell (the current test/build tooling is PowerShell-based; a POSIX-shell equivalent hasn't been written yet).

```powershell
# Build the release image (bootloader + kernel + user-space drivers)
powershell -File scripts/build-release.ps1

# Verify the release build is byte-for-byte reproducible from source
powershell -File scripts/verify-reproducible-build.ps1

# Run the full automated test suite (boot chain, fault injection, storage,
# every real driver, IOMMU containment, the shell, and the release image)
powershell -File scripts/test-integration.ps1
```

Individual test scripts (`scripts/test-*.ps1`) exercise one subsystem each — `test-boot`, `test-faults`, `test-keyboard`, `test-powerloss`, `test-ahci`, `test-nvme`, `test-e1000`, `test-shell`, `test-release`, and the driver-synthesis loop (`test-synthesis`, `test-synthesis-fault-demo`) among them. `test-host` runs the pure-logic unit tests (`host_tests/`) with no QEMU involved.

## Layout

| Path | What it is |
|---|---|
| `boot_rs/` | The UEFI bootloader |
| `kernel_rs/` | The kernel |
| `kernel_common/` | Pure, hardware-independent logic shared by the kernel and its host-side tests |
| `user_rs/` | User-space drivers and the shell, each a real ELF64 ring-3 process |
| `host_tests/` | Unit tests for `kernel_common`, run on the host with `cargo test`, zero QEMU |
| `scripts/` | Build and test automation |

## Status

Foundational work through hardware consolidation (memory management, capabilities, IOMMU-contained user-space drivers, filesystem, the agent runtime substrate, driver synthesis, the shell, and reproducible release builds) is complete and verified against QEMU. Multi-core (SMP) bring-up is in progress. Physical, non-emulated hardware validation is the one exit criterion that genuinely requires hands-on access to real silicon and has not yet been done.
