# Agentic OS

[![build-and-boot](https://github.com/pavann19/Agentic-OS/actions/workflows/ci.yml/badge.svg)](https://github.com/pavann19/Agentic-OS/actions/workflows/ci.yml)

Agentic OS is a from-scratch x86_64 kernel built around one idea: every
resource a process can touch — memory, files, windows, sockets,
devices — is reached through an explicit, typed capability, never
through ambient authority like a Unix UID. A process starts with
nothing; it can only do what it was explicitly handed, that grant can
be narrowed on the way to another process, and revoking it takes
effect immediately for every capability derived from it. The kernel
enforces this at the syscall boundary; drivers and services run as
ordinary, isolated, crash-restartable ring-3 processes outside it.
There is no AI or reasoning engine inside the kernel — automation
operates the machine from outside, through the same typed interfaces
any other program uses, never by screen-scraping or by running
inference in ring 0. It targets x86_64 only and does not attempt POSIX
or Windows compatibility; see `docs/DECISIONS.md` for why.

## Architecture

```text
              Applications / Agents
     shell · editor · file manager · net client
   ──────────────────────────────────────────────
        User-space drivers & system services
       (serial, storage, USB, net, display — each
        an isolated, crash-restartable process)
   ──────────────────────────────────────────────
                Capability ABI
        grant · derive · revoke · audit,
        enforced at the syscall boundary
   ──────────────────────────────────────────────
                    Kernel
     memory · scheduler · IPC · capabilities
   ──────────────────────────────────────────────
        Hardware — UEFI · IOMMU · TPM
```

Automated agents sit in the "Applications / Agents" row, not inside the
kernel — see `docs/DECISIONS.md` and `docs/ROADMAP.md` §1.3 for why
that boundary is permanent.

## Building and booting

Requires a nightly Rust toolchain and QEMU with OVMF firmware.
`rust-toolchain.toml` pins the exact nightly + targets this repo
builds against — `rustup show` in the repo root installs it.

### Linux

```bash
sudo apt-get install qemu-system-x86 ovmf
rustup show
make image
scripts/ci/boot-test.sh boot
```

### Windows

Use the GNU-ABI Rust toolchain, not MSVC — it needs no separate linker
or Build Tools install (`rustup toolchain install
nightly-<date>-x86_64-pc-windows-gnu`, matching the date in
`rust-toolchain.toml`).

```powershell
make image
powershell -File scripts/test-boot.ps1
```

### Running interactively

```bash
make run-qemu
```

### Full local regression suite (Windows only, 26 scripts)

```powershell
powershell -File scripts/test-integration.ps1
```

CI (`.github/workflows/ci.yml`) runs a smaller, honestly-scoped subset
of this on Linux on every push — see the next section for exactly
which subset.

## What's actually verified

`docs/VERIFICATION.md` is the full table. Summary: CI proves the
capability table, generation-based revocation, the syscall/IPC
boundary, address-space isolation, and UEFI boot on every push, on
Linux. Storage, network, display, USB, the app platform, and the
Live Agent Bridge are real, working code with their own passing local
test scripts, but aren't in CI yet — they're marked "Demonstrated," not
"Verified in CI," and the table says so rather than implying more than
what actually runs automatically.

## Demo

`scripts/record-boot-video.ps1` drives QEMU's real QMP protocol to
capture a genuine framebuffer frame sequence across a live boot
(verified this way: 30 real 640x480 frames, not synthetic). No
video file is committed to the repo — run the script yourself to
generate one (needs `ffmpeg` on PATH to encode to MP4; without it, the
script leaves the raw frame sequence as evidence).

## Limitations

- **QEMU only.** Nothing here has run on physical hardware. This is
  the single largest gap — see `docs/ROADMAP.md` §4.
- **CI covers the core scope, not everything.** See
  `docs/VERIFICATION.md` for what's Demonstrated-but-not-CI-checked vs.
  actually Verified in CI.
- **x86_64 only, Intel-chipset-modeled QEMU only.** AMD chipset support
  is not started; every IOMMU/driver path has only been validated
  against `q35`'s Intel-vendor virtual PCI IDs.
- **No POSIX/Windows compatibility**, deliberately — see
  `docs/DECISIONS.md`.
- **USB HID input is blocked** on an open Configure Endpoint bug; PS/2
  is the only tested input path.
- **Parser fuzzing is bounded, not continuous** — `fuzz/`'s CI job runs
  each target for 60 seconds per push, not an ongoing campaign.

## Further reading

- [`docs/VERIFICATION.md`](docs/VERIFICATION.md) — the full per-subsystem status table.
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — short-form rationale for the choices that most shape this codebase.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) — the full ADR-driven build plan.
- [`docs/PROGRESS.md`](docs/PROGRESS.md) — phase-by-phase implementation history.
- [`docs/SUPPORTED_HARDWARE.md`](docs/SUPPORTED_HARDWARE.md) — what's verified on real silicon vs. QEMU-only.

## License

Dual-licensed under [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option — the same convention Rust
itself uses.
