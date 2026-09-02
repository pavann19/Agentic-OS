# Supported Hardware

Phase 8, deliverable 4 (`docs/ROADMAP.md` §5): "Documented supported-hardware list — narrow and honest." This list states exactly what has been verified, on what, and with what evidence — nothing here is aspirational.

## Tier 1 — QEMU `q35` + OVMF + VirtIO (fully supported, fully verified)

The development and CI target. Every phase gate in this project (`docs/PROGRESS.md`) is validated here, with real evidence for every claim.

| Component | Status | Evidence |
|---|---|---|
| UEFI boot (OVMF firmware) | Supported | `scripts/test-boot.ps1` — real `BOOT_START → EXIT_BOOT_SERVICES_OK → KERNEL_ENTER` checkpoint chain, every run |
| Intel VT-d IOMMU (QEMU `intel-iommu` device) | Supported | Phase 3/6 — real DMAR discovery, translation-enabled root table, per-device domain assignment; real DMA-remapping fault capture verified in Phase 6 |
| Serial (COM1, 0x3F8) | Supported | Every driver's own boot log; Phase 7's shell reads it bidirectionally |
| PS/2 keyboard | Supported | `scripts/test-keyboard.ps1` — real synthetic keystroke via QEMU's own monitor, full IRQ1 → syscall → scancode path verified |
| GOP framebuffer | Supported | Phase 3 — real MMIO write, independently verified by a separate kernel-side readback |
| `virtio-blk` storage | Supported | Phase 4 — real ext2 filesystem, reboot-persistence, power-loss injection (`scripts/test-powerloss.ps1`, 7/7 trials) |
| AHCI (SATA) storage (`ich9-ahci`, 00:1f.2) | Supported (read path) | Phase 8 — real IDENTIFY DEVICE command, real device Model Number decoded and independently verified (`scripts/test-ahci.ps1`). Write path not yet implemented. |
| NVMe storage | Supported (admin/Identify path) | Phase 8 — real admin queue init + Identify Controller command, real device Model Number decoded (`scripts/test-nvme.ps1`). Matched by PCI class, not vendor ID. I/O queue pair / real block read-write not yet implemented. |
| `virtio-net` networking | Supported | Phase 6 — real driver synthesized from spec, real ARP frame independently pcap-verified, real bidirectional traffic observed |
| Intel e1000-class networking | Supported | Phase 8 — real MAC read from hardware registers, real ARP frame independently pcap-verified, real bidirectional traffic observed (`scripts/test-e1000.ps1`) |
| PCIe enumeration | Supported | Phase 3 — real config-space walk, `q35`'s real device topology |

**Not supported on Tier 1, by design:** legacy (non-modern) VirtIO transport, AMD-Vi (this kernel targets Intel VT-d only), SMP/multi-core (this kernel is single-core throughout).

## Tier 2 — physical hardware (selection done, physical bring-up NOT done)

**Recommended machine: Lenovo ThinkPad T480** (`docs/TIER2_HARDWARE.md`), selected against the roadmap's own four criteria — real Intel VT-d, a real coreboot-documented EC UART serial path, standard NVMe storage, a publicly documented Intel 200-series chipset. Any machine that genuinely satisfies those four criteria would work equally; the T480 is a researched recommendation, not a hard requirement.

**Honest status: NOT verified on any physical machine.** Every driver in this kernel has only ever been exercised against QEMU's own device emulation. Specifically, for real Tier 2 hardware:

| Component | Status |
|---|---|
| Storage (real NVMe/AHCI, not `virtio-blk`) | **Both AHCI and NVMe drivers now exist** (Phase 8, `user_rs/ahci_driver` + `user_rs/nvme_driver`) — real IDENTIFY DEVICE / Identify Controller verified against QEMU's own emulation of each. NVMe covers `docs/TIER2_HARDWARE.md`'s own researched PRIMARY-storage answer for the T480; AHCI covers a real secondary/legacy SATA path. **Honest gap remaining:** both are read/identify-only so far — no write path, no real block I/O queue pair for NVMe, and neither has been confirmed against real hardware. |
| Networking (real Intel-class NIC, not `virtio-net`) | **e1000-class driver now exists** (Phase 8, `user_rs/e1000_driver`) — real MAC read from hardware, real ARP frame independently pcap-verified against QEMU's own e1000 emulation. **Honest gap remaining:** not confirmed against real hardware; only ARP/TX exercised, no real IP-stack traffic. |
| Input (PS/2 keyboard) | Driver exists (Tier 1-verified) but never exercised on real PS/2 controller hardware — QEMU's emulation, however faithful, is not a substitute for confirming this on silicon. |
| Display (GOP framebuffer) | Driver exists (Tier 1-verified) but never exercised on real GPU/firmware framebuffer hardware. |
| IOMMU (real VT-d, not QEMU's emulation) | Kernel-side code is real and spec-driven, but real VT-d hardware has documented erratas and quirks no emulator reproduces — never confirmed against real silicon. |
| Boot chain over a real serial line | Never attempted — needs physical hardware, a wired serial connection, and a human present, none of which an AI agent has access to. |

**What closing this actually requires** is laid out in full in `docs/TIER2_HARDWARE.md` and `docs/PROGRESS.md`'s Phase 3 section: acquiring the machine, wiring a USB-to-TTL serial adapter to its EC UART header, building a bootable USB image from this project's own release artifacts (`scripts/build-release.ps1`), and booting it with a terminal program watching the serial line. None of that is code work; it is physical, human, one-time setup.

## What this list deliberately does NOT claim

- No claim of support for any hardware never listed above.
- No claim that Tier 1 (QEMU) verification predicts real-hardware behavior — it's a real, faithful emulator, but "faithful" is not "identical"; Tier 2's own existence in this roadmap is because of that gap, not despite it.
- No claim of SMP, AMD-Vi, or legacy BIOS support anywhere in this project.
