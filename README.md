# Agentic OS

Agentic OS is a from-scratch x86_64 operating system built around one idea: every resource a program can touch — memory, files, windows, network sockets, devices — is reached through an explicit, typed capability, never through ambient permission. There is no POSIX layer underneath it and no attempt to run existing Linux or Windows binaries. The system is written in Rust, boots on real UEFI firmware or under QEMU, and is designed so that an external program — human-driven or automated — can operate the machine through a real, typed interface instead of reading pixels off a screen.

## Why capabilities, not users and permissions

Conventional operating systems grant authority ambiently: a process running as a given user can touch anything that user is allowed to touch, and the kernel trusts that relationship implicitly. Agentic OS does the opposite. A process starts with nothing. Every file it can read, every window it can draw into, every network socket it can open exists as a capability object it was explicitly handed, and every use of that capability is checked against exactly the rights it was granted — not more. A capability can be derived into a narrower one (read-only from read-write, for instance), delegated to another process, or revoked outright, and every one of those events is written to an audit log that is itself a kernel primitive, not a service that could be quietly disabled.

The payoff is that a misbehaving or compromised process is contained by construction rather than by policy. A driver that crashes is restarted by a supervisor without taking the kernel down with it. A window can't read another window's framebuffer. A downloaded file can't be written anywhere the requesting process wasn't explicitly authorized to write.

## What's actually implemented

This isn't a toy kernel with a shell prompt. As of this writing the system has:

- A capability-based syscall ABI with per-process capability tables, grant/derive/revoke semantics, and a kernel-resident audit log.
- Preemptive multitasking across multiple cores, with real ACPI-driven CPU bring-up and a scheduler that survives crash-and-restart of individual drivers.
- Hardware-enforced DMA containment through Intel VT-d — every driver process is IOMMU-contained, not just address-space isolated.
- A real storage stack: ext2 with directory support, virtio-blk, AHCI, and NVMe, with power-loss injection testing to prove the filesystem recovers cleanly from an interrupted write.
- A real network stack: Ethernet, ARP, IPv4, ICMP, UDP, DNS, and TCP, verified against genuine external hosts, not just loopback traffic. Multiple network interfaces route by longest-prefix match.
- A capability-native display server. Windows are typed, capability-scoped objects; a client process never touches the framebuffer directly. GPU-accelerated 2D composition is available where the hardware supports it, with a software fallback everywhere else.
- USB host controller support (xHCI) with HID keyboard and mouse, PS/2 fallback, and hot-plug handling.
- A native application platform: a capability manifest format enforced at install time, a package store with signed updates and rollback, and a small set of reference applications (a terminal, a text editor, a file manager, a network client) built entirely against the public ABI.
- Multi-user sessions where isolation between users is enforced the same way isolation between any two processes is — through capabilities, not UID checks bolted on afterward.
- Secure boot with a measured boot chain into a TPM event log, full-disk encryption, and cryptographically signed updates with dual-bank rollback — all implemented as pure, dependency-free Rust, no external crypto library.
- A typed interface for driving the system programmatically: window state, input injection, and process control are all exposed as real syscalls with real capability checks, not screen-scraping.

None of this runs model inference or automated reasoning inside the kernel, and it never will — that's a permanent design boundary, not a current limitation. The kernel's job is memory, scheduling, isolation, and capability enforcement, full stop. Anything resembling decision-making lives entirely outside it, talking to the machine through the same typed interface any other program would use.

## Building and running

The build is native — no Docker, no cross-platform build container. You need:

- A nightly Rust toolchain (`rustup`) targeting both `x86_64-unknown-none` (the kernel and ring-3 processes) and `x86_64-unknown-uefi` (the bootloader).
- QEMU with OVMF/EDK2 firmware for the UEFI boot path.

On Windows, use the GNU-ABI Rust toolchain rather than MSVC — it needs no separate linker or Build Tools installation, which keeps the dependency list to just `rustup` and QEMU.

Build everything and assemble a bootable image:

```bash
make image
```

Boot it interactively, with hardware-accelerated virtualization where the host supports it:

```bash
make run-qemu
```

Run the automated regression suite (dozens of independent boot-and-verify scripts covering every subsystem above):

```powershell
powershell -File scripts/test-integration.ps1
```

A single headless boot smoke test, useful for a quick sanity check:

```bash
make test-boot
```

For a graphical session with the reference applications running in a desktop layout:

```powershell
powershell -File scripts/run-gui.ps1 -Mode launcher
```

If you're testing interactively in a graphical QEMU window, click inside the display area first and confirm the window title shows a keyboard-grab indicator — otherwise keystrokes go to your host window manager instead of the guest.

## Repository layout

- `boot_rs/` — the UEFI bootloader.
- `kernel_rs/` — the kernel itself: memory management, scheduling, capabilities, drivers, IPC, the display server.
- `kernel_common/` — code shared between the kernel and the bootloader, including the cryptographic primitives.
- `user_rs/` — ring-3 processes: drivers, the shell, and the reference applications.
- `host_tests/` — pure-logic unit tests that run on the host toolchain, no QEMU required.
- `scripts/` — the test and build automation, one script per subsystem, all wired into the full regression suite.
- `docs/` — the design record: `ROADMAP.md` holds the architecture decisions and their rationale, `PROGRESS.md` holds a dated account of what's been built and verified, `SUPPORTED_HARDWARE.md` states plainly what's been tested and on what.

## Evidence, not claims

Every subsystem listed above has a corresponding test script that boots a real QEMU instance, exercises the real code path, and checks the resulting serial output against specific expected markers — including deliberately adversarial cases, like a process attempting an action it was never granted the capability for. `docs/PROGRESS.md` records what's actually been verified, when, and against what evidence, and says plainly where something hasn't been tested yet rather than implying it has. Hardware support is documented the same way in `SUPPORTED_HARDWARE.md`: what's been run on real silicon, what's only been run under QEMU, and what's a stated target with no evidence behind it yet.

## What this isn't

There's no path to running unmodified Linux or Windows binaries, and there won't be — a compatibility shim would mean reintroducing the ambient authority the whole capability model exists to avoid. There's no multi-architecture support planned. The project targets one real, honest goal: an operating system that can build its own next release while running on itself, survive continuous real-world use, and install from nothing onto real hardware into something genuinely usable. Where the project stands against that goal, and what's still open, is tracked in `docs/ROADMAP.md`.
