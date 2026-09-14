# Tier 3 Hardware Qualification & Platform Guide

Phase 11, Deliverable 1 (`docs/ROADMAP.md` §5 and ADR-007): Bounded Tier 3 hardware qualification matrix spanning multiple form factors and chipset generations.

This document details the hardware specifications, firmware configuration requirements, IOMMU DMA protection topologies, serial telemetry setups, and power management qualifications for Agentic OS deployments on physical Tier 3 systems.

---

## 1. Architectural Invariants

Every machine qualified in Tier 3 must strictly adhere to the foundational security and reliability invariants of Agentic OS:

1. **Hardware-Enforced DMA Containment (ADR-006)**:
   - Must incorporate functional **Intel VT-d (Virtualization Technology for Directed I/O)**.
   - ACPI tables must expose valid `DMAR` reporting DRHD (DMA Remapping Hardware Definition) units.
   - Device DMA is bounded to assigned driver buffers via page-table remapping. Any unauthorized DMA access immediately triggers a hardware fault captured by the kernel fault monitor.

2. **USB xHCI & HID Subsystem (ADR-007 & Phase 11)**:
   - Must feature a standard compliant **xHCI (eXtensible Host Controller Interface 1.0, 1.1, or 1.2)**.
   - Peripheral input (keyboard and mouse) operates via user-space ring-3 driver (`user_rs/usb_xhci_driver`), parsing standard USB HID boot-protocol reports into kernel `input_routing`.
   - Dynamic USB hot-plug support handles runtime connect/disconnect events, allocating and disabling slots without kernel disruption.

3. **Storage & Networking**:
   - Storage: Standard PCIe M.2 NVMe SSD (PCI class `01:08:02`) or SATA AHCI controller (PCI class `01:06:01`).
   - Network: Intel Gigabit Ethernet (e1000e family, supported by `user_rs/e1000_driver` and `user_rs/netstack_driver`).

4. **ACPI Power Management (Phase 11.3)**:
   - ACPI `FADT` (`FACP`) table must expose standard power management registers (`PM1a_CNT_BLK`, `PM_TMR_BLK`).
   - Supports CPU low-power idle states (C1 via `hlt`, C1E via `mwait`).
   - Supports S3 Suspend-to-RAM state machine with architectural register capture and memory integrity preservation.

5. **Diagnostic Telemetry (Serial COM / EC UART)**:
   - Early boot and ring-3 driver diagnostics require an unbuffered serial channel at `0x3F8` (115200 baud, 8-N-1) to ensure deterministic logging without relying on graphical surfaces.

---

## 2. Qualified Hardware Matrix

### Platform 1: Dell OptiPlex 7050 / 7060 / 7070 (Desktop / Micro SFF)

The Dell OptiPlex series represents the enterprise desktop target, offering robust build quality, readily accessible spare parts, and integrated physical COM headers.

| Subsystem | Specification |
|---|---|
| **Form Factor** | Small Form Factor (SFF) or Micro PC |
| **Chipset** | Intel Q270 (7050) / Intel Q370 (7060 / 7070) |
| **Processor** | Intel Core i5-7500 / i7-8700 / i7-9700 (65W/35W) |
| **Memory** | DDR4-2400 / DDR4-2666 non-ECC (up to 64GB) |
| **IOMMU (VT-d)** | Intel VT-d supported on Q270/Q370 PCH with 2 DRHD units (integrated graphics + PCIe) |
| **USB Host** | Intel Sunrise Point / Cannon Lake xHCI USB 3.0 / USB 3.1 Controller |
| **Storage** | 1x M.2 2280 PCIe 3.0 x4 NVMe + 1x SATA 6Gbps AHCI |
| **Network** | Intel I219-LM Gigabit Ethernet PHY (PCI device `00:1f.6`) |
| **Serial Header** | Motherboard 9-pin COM1 header (`J_SERIAL` / `COM_A`), maps to standard IO `0x3F8` |
| **Firmware** | Dell UEFI BIOS with selectable AHCI and VT-d toggles |

#### Firmware Configuration Checklist:
- **Virtualization Support** → **Intel Virtualization Technology**: `Enabled`
- **Virtualization Support** → **VT for Direct I/O**: `Enabled`
- **System Configuration** → **SATA Operation**: `AHCI` (do not use RAID ON / Intel RST)
- **System Configuration** → **Serial Port**: `COM1 (IO: 3F8h, IRQ 4)`
- **Power Management** → **Deep Sleep Control**: `Disabled` (ensures S3 Suspend-to-RAM operates reliably)
- **Secure Boot** → **Secure Boot Enable**: `Disabled` (or enroll Agentic OS custom PK/KEK keys)

---

### Platform 2: Lenovo ThinkPad T480 / T490 (Laptop)

The ThinkPad T480/T490 serves as the mobile reference target. It features open documentation, widespread coreboot development, and dual-battery power architecture.

| Subsystem | Specification |
|---|---|
| **Form Factor** | 14-inch Business Laptop |
| **Chipset** | Intel 300-series Mobile (Cannon Lake-LP / Whiskey Lake) |
| **Processor** | Intel Core i5-8250U / i7-8550U / i5-8265U / i7-8665U |
| **Memory** | Dual-channel DDR4 SO-DIMM (up to 64GB) |
| **IOMMU (VT-d)** | Full DMA remapping and interrupt remapping supported across all buses |
| **USB Host** | Intel Cannon Lake-LP USB 3.1 Gen 2 xHCI Controller (Type-A and Type-C Thunderbolt) |
| **Storage** | M.2 2280 NVMe PCIe 3.0 x4 SSD |
| **Network** | Intel I219-V Gigabit Ethernet + Intel Wireless-AC 8265 |
| **Serial Debug** | Embedded Controller (EC UART) debug test points accessible under keyboard |
| **Power Management** | ACPI 6.0 Smart Battery Subsystem, CPU C-states (C1/C1E/C6/C8), S3 Sleep |

#### Firmware Configuration Checklist:
- **Security** → **Virtualization** → **Intel (R) Virtualization Technology**: `Enabled`
- **Security** → **Virtualization** → **Intel (R) VT-d Feature**: `Enabled`
- **Config** → **Power** → **Sleep State**: `Linux (S3)` (switch from Windows S0ix Modern Standby)
- **Config** → **Thunderbolt 3** → **Security Level**: `DisplayPort and USB Only` (enables IOMMU DMA containment on external ports)
- **Startup** → **UEFI/Legacy Boot**: `UEFI Only`, **CSM Support**: `No`

---

### Platform 3: HP EliteDesk 800 G4 / G5 Desktop Mini (Mini PC)

The HP EliteDesk Mini provides an ultra-compact, high-performance platform ideal for headless appliance or low-footprint agent nodes.

| Subsystem | Specification |
|---|---|
| **Form Factor** | Desktop Mini (1 Liter chassis) |
| **Chipset** | Intel Q370 vPro Chipset |
| **Processor** | Intel Core i5-8500T / i7-8700T / i5-9500T (35W low-power) |
| **Memory** | Dual-channel DDR4-2666 SO-DIMM |
| **IOMMU (VT-d)** | Intel VT-d supported natively in HP Sure Start firmware |
| **USB Host** | Intel Q370 xHCI Controller (6x USB 3.1 Type-A + 1x USB 3.1 Type-C front) |
| **Storage** | Dual M.2 2280 PCIe NVMe SSD slots |
| **Network** | Intel I219-LM Gigabit Ethernet |
| **Serial Port** | Optional factory rear DB-9 COM1 port (`0x3F8`) or internal 9-pin serial header |
| **Power Management** | ACPI FADT compliant, idle low-power states, S3 sleep supported |

#### Firmware Configuration Checklist:
- **Advanced** → **System Options** → **Virtualization Technology (VTx)**: `Enabled`
- **Advanced** → **System Options** → **Virtualization Technology for Directed I/O (VTd)**: `Enabled`
- **Advanced** → **Device Options** → **Serial Port A**: `Enabled (IO: 3F8h, IRQ 4)`
- **Advanced** → **Storage Options** → **SATA Emulation**: `AHCI`
- **Security** → **Secure Boot Configuration** → **Legacy Support**: `Disabled`, **Secure Boot**: `Disabled`

---

## 3. Physical Bring-up & Qualification Checklist

When validating a new machine from the Tier 3 matrix:

1. **Bootable Media Creation**:
   Generate a bootable USB drive using the reproducible release pipeline:
   ```powershell
   powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
   ```
   Write the resulting FAT32 filesystem onto a USB flash drive (GUID Partition Table with EFI System Partition).

2. **Hardware Serial Link Verification**:
   - Connect a USB-to-RS232 adapter (FTDI / CP2102) to the COM1 port/header on the target machine.
   - Open a terminal client (e.g., PuTTY, Minicom) configured to `115200 bps, 8 data bits, no parity, 1 stop bit, no flow control`.
   - Power on the target machine. Observe early boot checkpoint chain:
     ```
     BOOT_START
     EXIT_BOOT_SERVICES_OK
     KERNEL_ENTER
     ACPI_INIT_START
     ACPI_INIT_DONE
     ACPI_DMAR_FOUND
     IOMMU_INIT_START
     IOMMU_INIT_DONE
     ```

3. **Subsystem Validation**:
   - **IOMMU Containment**: Assert that `iommu::init` discovers the DMAR units and enables translation.
   - **USB xHCI & HID**: Connect a USB keyboard and mouse. Verify that `usb_xhci_driver` discovers root ports, allocates slots, configures endpoints, and decodes key strokes and mouse reports into `input_routing`.
   - **Storage**: Verify that NVMe / AHCI controllers identify media model numbers without timeouts.
   - **Power Management**: Execute S3 suspend cycle via power button or ACPI request; observe system entry into sleep and resumption with verified byte-identical memory integrity.
