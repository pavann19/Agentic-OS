# Agentic OS

**Agentic OS** is a from-scratch **x86_64 operating system built around capability-based security**.

Its core principle is simple:

> **Every resource a program can access is reached through an explicit, typed capability — never through ambient authority.**

Memory, files, windows, network sockets, devices, and other system resources are exposed through capabilities that are explicitly granted, narrowed, delegated, or revoked.

Agentic OS does **not** implement POSIX compatibility and does **not** attempt to execute existing Linux or Windows binaries. The system is written in **Rust**, boots on real **UEFI firmware** or under **QEMU**, and exposes a typed programmatic interface that allows both humans and automated software to operate the machine without relying on screen scraping or pixel-level interaction.

---

## Core Philosophy

### Capabilities Instead of Ambient Permissions

Traditional operating systems generally associate authority with identities such as users, groups, and process credentials.

Agentic OS takes a different approach.

A process starts with **no authority**.

If a process needs to:

* read a file,
* write to storage,
* create or control a window,
* communicate over a network socket,
* access a device,
* interact with another process,

it must first receive the corresponding **capability**.

Each capability explicitly defines what the holder is allowed to do.

For example:

```text
Process
   │
   ├── File Capability
   │      └── Read
   │
   ├── Window Capability
   │      ├── Draw
   │      └── Present
   │
   └── Network Capability
          └── UDP Send
```

Capabilities can be:

* **Granted** — explicitly transferred to a process.
* **Derived** — converted into a narrower capability.
* **Delegated** — passed to another process.
* **Revoked** — invalidated when authority is no longer required.
* **Audited** — security-relevant operations are recorded by the kernel.

For example, a read-write file capability can be derived into a read-only capability without granting the recipient any additional authority.

### Why This Matters

The goal is to make isolation a property of the system architecture rather than a collection of policies layered on top of it.

A compromised process should not become powerful simply because it compromised another process.

Examples:

* A window cannot access another window's framebuffer.
* A driver cannot DMA into arbitrary physical memory.
* A downloaded file cannot be written outside its granted storage capabilities.
* A process cannot open a network endpoint without an appropriate network capability.
* A crashed driver can be restarted without bringing down the kernel.

The kernel therefore acts as the **enforcement boundary**, while higher-level services remain isolated and replaceable.

---

# Architecture

Agentic OS is structured around a small, strongly enforced kernel and capability-native user-space services.

```text
┌─────────────────────────────────────────────┐
│             Applications / Agents           │
├─────────────────────────────────────────────┤
│       Shell • Editor • File Manager         │
│          Network & Reference Apps           │
├─────────────────────────────────────────────┤
│      User-Space Drivers & System Services   │
├─────────────────────────────────────────────┤
│              Capability ABI                 │
├─────────────────────────────────────────────┤
│                    Kernel                   │
│                                             │
│  Memory • Scheduler • IPC • Capabilities    │
│  Storage • Network • USB • Display • ACPI   │
├─────────────────────────────────────────────┤
│          Hardware / UEFI / TPM / IOMMU      │
└─────────────────────────────────────────────┘
```

The kernel deliberately has a narrow responsibility:

> **Memory, scheduling, isolation, IPC, hardware control, and capability enforcement.**

Automated reasoning, model inference, and decision-making do **not** belong inside the kernel.

That boundary is intentional and permanent.

Any intelligent or automated system operates outside the kernel and interacts with the operating system through the same typed interfaces available to ordinary applications.

---

# What's Implemented

Agentic OS is substantially more than a bootable kernel or shell prompt.

The current system includes the following major subsystems.

## Capability-Based Syscall ABI

* Per-process capability tables.
* Typed capability objects.
* Capability grant operations.
* Capability derivation.
* Capability revocation.
* Kernel-resident audit logging.
* Explicit authorization checks for resource access.

The capability model is enforced at the syscall boundary rather than implemented as an optional user-space convention.

---

## Multicore Preemptive Kernel

* Preemptive multitasking.
* Multicore scheduling.
* ACPI-driven CPU bring-up.
* Process and thread management.
* Driver crash isolation.
* Driver restart supervision.

Individual driver failures are designed to be recoverable without requiring a complete kernel restart.

---

## Hardware-Enforced DMA Isolation

Agentic OS uses **Intel VT-d** to provide IOMMU-based DMA containment.

Driver processes are therefore isolated at two levels:

```text
CPU / MMU isolation
        +
IOMMU / DMA isolation
```

A compromised driver should not be able to arbitrarily DMA into memory belonging to the kernel or another process.

---

## Storage Stack

The operating system includes a functional storage subsystem with:

* ext2 filesystem support.
* Directory operations.
* virtio-blk.
* AHCI.
* NVMe.
* Power-loss testing.
* Interrupted-write recovery testing.

Filesystem testing includes deliberately interrupted writes to verify recovery behavior rather than relying exclusively on clean shutdown scenarios.

---

## Network Stack

The networking subsystem currently supports:

* Ethernet.
* ARP.
* IPv4.
* ICMP.
* UDP.
* DNS.
* TCP.
* Multiple network interfaces.
* Longest-prefix routing.

Networking is tested against **real external hosts**, rather than being limited to loopback or synthetic traffic.

---

## Capability-Native Display Server

The graphical subsystem treats windows as typed, capability-scoped objects.

Applications do **not** receive direct access to the framebuffer.

Instead:

```text
Application
     │
     │ Window Capability
     ▼
Display Server
     │
     ▼
Compositor
     │
     ▼
Display Hardware
```

Features include:

* Capability-scoped windows.
* Window state management.
* Input handling.
* Process-controlled graphical interfaces.
* GPU-accelerated 2D composition where supported.
* Software rendering fallback.

This also provides a typed interface that can be used by automated software without interpreting screenshots.

---

## USB and Input

USB host-controller support includes:

* xHCI.
* HID keyboard.
* HID mouse.
* Hot-plug handling.
* PS/2 fallback.

The system can therefore operate with both modern USB input devices and legacy PS/2 hardware.

---

## Native Application Platform

Agentic OS provides a native application model rather than attempting to reproduce an existing desktop operating-system API.

It includes:

* Capability manifests.
* Install-time capability enforcement.
* A package store.
* Signed package updates.
* Rollback support.
* Native reference applications.

Reference applications include:

* Terminal.
* Text editor.
* File manager.
* Network client.

All are built against the public Agentic OS ABI.

---

## Multi-User Sessions

Multiple users can have independent sessions while retaining the same underlying capability model.

User isolation is therefore not implemented as an unrelated UID-based security layer.

Instead, the same capability mechanisms used for process isolation are used to establish authority boundaries between sessions.

---

## Secure Boot and Platform Security

The security architecture includes:

* Secure Boot.
* Measured boot.
* TPM event logging.
* Full-disk encryption.
* Cryptographically signed system updates.
* Dual-bank update architecture.
* Automatic rollback support.

Cryptographic primitives are implemented in Rust without relying on an external cryptographic library.

---

# Programmatic Control

One of the defining properties of Agentic OS is that the operating system is designed to be **machine-operable through typed interfaces**.

System state and control operations are exposed through actual OS interfaces rather than requiring an external program to interpret pixels.

The interface includes operations for:

* Window state.
* Input injection.
* Process control.
* Capability management.
* Resource access.

Conceptually:

```text
Automated Program
       │
       │ Typed API / Syscalls
       ▼
Agentic OS
       │
       ├── Processes
       ├── Windows
       ├── Files
       ├── Network
       └── Devices
```

This makes automation a first-class interaction model without putting an AI or reasoning engine inside the kernel.

---

# AI and Intelligence Boundary

Agentic OS is **not an AI model running inside an operating-system kernel**.

There is a deliberate architectural boundary between:

### Kernel

Responsible for:

* Memory.
* Scheduling.
* Isolation.
* IPC.
* Hardware.
* Capabilities.
* Security enforcement.

### Intelligence Layer

Responsible for:

* Reasoning.
* Planning.
* Decision-making.
* Natural-language interaction.
* Automation.
* Higher-level task execution.

An intelligent system can operate the machine through the same typed interfaces available to conventional programs.

This separation keeps the trusted computing base small while allowing increasingly sophisticated automation to be built outside the kernel.

---

# Building and Running

Agentic OS uses a **native build environment**.

There is no Docker-based build container or cross-platform development environment required.

## Requirements

You need:

* A nightly Rust toolchain (`rustup`), targeting both `x86_64-unknown-none` (the kernel and ring-3 processes) and `x86_64-unknown-uefi` (the bootloader).
* QEMU with OVMF/EDK2 firmware for the UEFI boot path.

On Windows, use the GNU-ABI Rust toolchain rather than MSVC — it needs no separate linker or Build Tools installation.

---

## Build a Bootable Image

Build the complete system and assemble a bootable image:

```bash
make image
```

---

## Run Under QEMU

Launch Agentic OS interactively:

```bash
make run-qemu
```

The QEMU configuration uses hardware-accelerated virtualization where supported by the host.

---

## Run the Integration Suite

Run the complete automated regression suite:

```powershell
powershell -File scripts/test-integration.ps1
```

The suite performs independent boot-and-verify tests across the major operating-system subsystems.

---

## Boot Smoke Test

For a quick headless sanity check:

```bash
make test-boot
```

---

## Launch the Graphical Environment

To start a graphical session with the reference applications:

```powershell
powershell -File scripts/run-gui.ps1 -Mode launcher
```

### QEMU Input Note

When testing interactively inside a graphical QEMU window, click inside the guest display first.

Confirm that the QEMU window indicates that keyboard input is captured. Otherwise, keystrokes may be sent to the host window manager instead of the guest operating system.

---

# Repository Structure

```text
Agentic OS/
│
├── boot_rs/
│   └── UEFI bootloader
│
├── kernel_rs/
│   └── Kernel
│       ├── Memory management
│       ├── Scheduler
│       ├── Capabilities
│       ├── IPC
│       ├── Drivers
│       └── Display server
│
├── kernel_common/
│   └── Shared kernel/bootloader code
│       └── Cryptographic primitives
│
├── user_rs/
│   └── Ring-3 software
│       ├── Drivers
│       ├── Shell
│       └── Applications
│
├── host_tests/
│   └── Host-side pure-logic tests
│
├── scripts/
│   └── Build and integration automation
│
├── docs/
│   ├── ROADMAP.md
│   ├── PROGRESS.md
│   └── SUPPORTED_HARDWARE.md
│
└── README.md
```

### Directory Overview

| Directory        | Purpose                                                    |
| ---------------- | ---------------------------------------------------------- |
| `boot_rs/`       | UEFI bootloader                                            |
| `kernel_rs/`     | Core kernel implementation                                 |
| `kernel_common/` | Shared bootloader/kernel code and cryptographic primitives |
| `user_rs/`       | Ring-3 drivers, shell, and applications                    |
| `host_tests/`    | Host-side unit and logic tests                             |
| `scripts/`       | Build, boot, and integration automation                    |
| `docs/`          | Architecture, progress, and hardware documentation         |

---

# Testing Philosophy

## Evidence, Not Claims

Agentic OS follows an **evidence-first development model**.

Subsystems are not considered complete simply because the implementation compiles or appears to work interactively.

Where practical, each subsystem has automated tests that:

1. Build the relevant components.
2. Boot a real QEMU instance.
3. Exercise the actual code path.
4. Collect serial output.
5. Check for explicit expected markers.
6. Test failure and adversarial cases where applicable.

For example, capability testing includes processes deliberately attempting operations for which they were never granted authority.

A successful test therefore demonstrates not only that the intended operation works, but also that unauthorized operations are rejected.

---

## Progress Tracking

[`docs/PROGRESS.md`](docs/PROGRESS.md) records:

* What has been implemented.
* What has been tested.
* When it was tested.
* The environment in which it was tested.
* The evidence supporting the result.
* Known gaps.

If a subsystem has not been tested, it is documented as **untested** rather than presented as complete.

---

## Hardware Support

[`docs/SUPPORTED_HARDWARE.md`](docs/SUPPORTED_HARDWARE.md) distinguishes between:

* Hardware tested on real silicon.
* Hardware tested under QEMU.
* Hardware that is currently supported by implementation but not yet physically validated.
* Future hardware targets.

This distinction is intentional.

> **A feature without evidence is a target, not a verified capability.**

---

# Design Principles

Agentic OS is built around several long-term principles.

### 1. Explicit Authority

Processes receive only the resources they explicitly need.

### 2. Least Privilege

Capabilities can be narrowed so that a process receives the minimum authority required for an operation.

### 3. Revocability

Authority should not have to remain permanent once it is no longer needed.

### 4. Isolation by Construction

Security boundaries should be enforced by the architecture rather than depending solely on application behavior.

### 5. Small Trusted Core

The kernel should remain responsible for a limited set of security-critical operations.

### 6. Typed Interfaces

Machine interaction should use explicit interfaces and structured state rather than screen scraping.

### 7. Replaceable Services

User-space services and drivers should be restartable without requiring the entire operating system to restart.

### 8. Evidence-Driven Development

Claims of functionality should be backed by reproducible tests or documented hardware evidence.

### 9. No AI in the Kernel

Reasoning and intelligence belong outside the trusted kernel boundary.

---

# What Agentic OS Is Not

## Not a Linux Distribution

Agentic OS is not based on Linux and does not provide a POSIX compatibility layer.

There is no attempt to reproduce Linux system calls simply to execute existing Linux software.

---

## Not a Windows Replacement

The system does not attempt to execute existing Windows binaries or reproduce the Windows API.

---

## Not a Compatibility Layer

Compatibility with existing operating systems is intentionally outside the project's architecture.

A compatibility subsystem that recreated broad ambient authority would undermine the security model that Agentic OS is designed around.

---

## Not an AI Kernel

The operating system does not run model inference or autonomous reasoning inside the kernel.

AI and automation operate as external software and communicate with the OS through typed, capability-checked interfaces.

---

## Not a Multi-Architecture OS

The project currently targets **x86_64**.

There is no current plan to make the kernel multi-architecture.

---

# Long-Term Goal

The ultimate objective is not merely to create a kernel that boots.

The goal is to build an operating system that can:

1. **Build its own next release.**
2. **Run continuously on itself.**
3. **Recover from failures in individual services and drivers.**
4. **Operate on real hardware.**
5. **Provide strong capability-based isolation.**
6. **Support real-world workloads.**
7. **Install from a clean machine into a genuinely usable system.**
8. **Expose the entire machine through reliable, typed interfaces suitable for both humans and automated software.**

The project is therefore progressing toward a **self-hosting, capability-native, automation-friendly operating system**, rather than a demonstration kernel.

---

# Project Status

The authoritative project status is maintained in:

* [`docs/ROADMAP.md`](docs/ROADMAP.md) — architecture decisions, priorities, and remaining work.
* [`docs/PROGRESS.md`](docs/PROGRESS.md) — implementation and verification history.
* [`docs/SUPPORTED_HARDWARE.md`](docs/SUPPORTED_HARDWARE.md) — verified hardware support.

These documents should be treated as the source of truth for what is currently implemented, tested, and still under development.

---

# License

Licensed under either of:

* [Apache License, Version 2.0](LICENSE-APACHE)
* [MIT License](LICENSE-MIT)

at your option, following the same dual-licensing convention as the Rust project itself.

---

## Summary

**Agentic OS is an operating system designed around explicit authority, strong isolation, typed machine interfaces, and a deliberately small trusted kernel.**

It does not attempt to retrofit agentic behavior into an existing operating system.

Instead, it provides the foundation on which automated software can safely operate a machine:

```text
              ┌──────────────────────┐
              │   Human / AI / Agent │
              └──────────┬───────────┘
                         │
                  Typed Interfaces
                         │
              ┌──────────▼───────────┐
              │     Agentic OS       │
              │                      │
              │ Capabilities         │
              │ Isolation             │
              │ Scheduling             │
              │ Memory                │
              │ Devices               │
              └──────────┬───────────┘
                         │
              ┌──────────▼───────────┐
              │       Hardware       │
              └──────────────────────┘
```

**Explicit authority. Typed interfaces. Isolated services. Evidence-backed engineering.**
