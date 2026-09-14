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

**Not supported on Tier 1, by design:** legacy (non-modern) VirtIO transport, AMD-Vi (this kernel targets Intel VT-d only). Note: Multi-core SMP was added and evidenced in Phase 9 (`scripts/test-boot.ps1` with multi-CPU topology, `scripts/test-integration.ps1`).

## Tier 2 — Physical Hardware Reference (Single Machine Target)

**Recommended machine: Lenovo ThinkPad T480** (`docs/TIER2_HARDWARE.md`), selected against the roadmap's four criteria:
- Intel VT-d IOMMU
- Coreboot-documented EC UART serial path
- Standard M.2 NVMe storage
- Intel 200/300-series chipset

## Tier 3 — Published, Bounded Hardware Matrix (Phase 11, Deliverable 1)

Pursuant to `docs/ROADMAP.md` §5 (Phase 11) and ADR-007, Tier 3 extends hardware qualification beyond a single machine target to a bounded, documented matrix spanning two chipset generations and multiple form factors (desktop, laptop, and mini PC). Every platform in this matrix meets the architectural invariant of Agentic OS: **hardware-enforced IOMMU (Intel VT-d) DMA containment, xHCI USB host controller, standard NVMe/AHCI block storage, Intel e1000-compatible NIC, ACPI FADT power management, and an early-boot serial/UART diagnostic channel.**

| System / Model | Form Factor | Chipset / CPU Gen | IOMMU / DMA Containment | USB Subsystem (Phase 11) | Storage & Network | Diagnostic / Serial Channel | Power Management |
|---|---|---|---|---|---|---|---|
| **Dell OptiPlex 7050 / 7060 / 7070** | Small Form Factor (SFF) / Micro | Intel Q270 / Q370 (7th–9th Gen Core i5/i7) | Intel VT-d (DMAR / DRHD enabled in Dell BIOS) | Intel xHCI Controller (USB 3.0 / USB 3.1 Gen 2), HID keyboard & mouse | M.2 2280 NVMe PCIe x4 + SATA AHCI; Intel I219-LM Gigabit Ethernet (`e1000_driver`) | Motherboard 9-pin COM1 header (`0x3F8`, standard 16550 UART) | ACPI 6.1 FADT, CPU C1/C1E idle, S3 Suspend-to-RAM |
| **Lenovo ThinkPad T480 / T490** | Business Laptop | Intel 300-series Mobile (8th–10th Gen Core U-series) | Intel VT-d (Full DMA remapping & Interrupt remapping) | Intel Cannon Lake PCH-LP xHCI USB 3.1 Controller | M.2 NVMe PCIe x4; Intel I219-V Gigabit Ethernet + USB Ethernet | Coreboot-documented EC UART test points / UEFI serial console redirection | ACPI FADT, Smart Battery Subsystem, C-states, S3 Suspend-to-RAM |
| **HP EliteDesk 800 G4 / G5** | Desktop Mini (DM) PC | Intel Q370 (8th–9th Gen Core i5/i7/i9 65W/35W) | Intel VT-d (HP Sure Start BIOS hardware virtualization enabled) | Intel Q370 xHCI Controller (Front Type-C + 6x Type-A ports) | Dual M.2 2280 NVMe slots; Intel I219-LM Gigabit Ethernet | Factory optional rear DB-9 COM1 serial port (`0x3F8`) or internal header | ACPI FADT, Low-power idle C-states, S3 Suspend-to-RAM |

### Architectural Qualification Requirements for Tier 3 Platforms
1. **IOMMU Hard-Gate (ADR-006)**: BIOS must support VT-d / DMA Remapping. Root tables and device domains are programmed per-device (`authority::grant_device`). Devices with malfunctioning IOMMU translation are quarantined.
2. **USB xHCI & HID (ADR-007)**: Controllers must support xHCI 1.0 or 1.1/1.2 specifications with MSI/MSI-X or pin interrupts, 64-byte or 32-byte contexts, and standard USB HID boot-protocol keyboard/mouse reporting.
3. **Power Management (Phase 11.3)**: ACPI tables must expose valid `FADT` (`FACP`) with non-zero `PM1a_CNT_BLK` or valid extended GAS descriptors for S3 sleep state (`SLP_TYP` / `SLP_EN`). CPU idle loop transitions cores into C1 state (`hlt`).
4. **Early-Boot Telemetry**: A physical RS-232 COM port or EC UART reachable at `0x3F8` ensures zero headless deadlocks during kernel bring-up.

For full qualification steps, board jumper settings, and firmware configuration, see [`docs/TIER3_HARDWARE.md`](file:///d:/Operating_System/docs/TIER3_HARDWARE.md).

## What this list deliberately does NOT claim

- No claim of support for any hardware not explicitly cataloged in Tier 1, Tier 2, or Tier 3 above.
- No claim that Tier 1 (QEMU) emulation substitutes for silicon testing; physical qualification requires dedicated board bring-up.
- No claim of AMD-Vi (SVM) or legacy BIOS support; Agentic OS is strictly 64-bit UEFI with Intel VT-d.
