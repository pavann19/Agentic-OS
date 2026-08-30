# Tier 2 Hardware Selection

Phase 3's last item (`docs/ROADMAP.md` §4/§7): "one specific physical
machine, chosen at Phase 3." Selection criteria, in the roadmap's own
order: (a) IOMMU present and functional (ADR-006 makes this
non-negotiable), (b) serial output reachable — physical port or
USB-serial with a working early-boot path, (c) NVMe or AHCI storage,
(d) publicly documented chipset.

**Honesty note up front, stated plainly rather than glossed over:** this
document completes the *selection* half of this item — real research,
with real sources, against the roadmap's own four criteria. It does
**not** complete the *"brought to serial output"* half. That requires
physically owning the machine, connecting a serial adapter, and running
this kernel's actual boot chain on it — none of which an AI agent without
hardware access can do. This item stays open (not checked off) in
`PROGRESS.md` until that physical step actually happens.

## Recommendation: Lenovo ThinkPad T480

| Criterion | How the T480 meets it | Source |
|---|---|---|
| (a) IOMMU present | Intel VT-d, standard on the T480's Kaby Lake-R / Coffee Lake-U mobile Core i5/i7 CPU options (e.g. i5-8350U, i7-8650U vPro) — VT-d has been standard across this whole Intel U-series mobile tier since well before this generation, with vPro-branded SKUs in particular requiring it as part of the vPro feature set. **Not independently re-verified against a live Intel ARK page in this session** (ark.intel.com blocked automated fetches during this research) — confirm the exact SKU's VT-d line on ark.intel.com before purchase; this is a strong, well-established expectation, not a substitute for that final check. | Community/vPro-tier consensus; verify per-SKU on ark.intel.com |
| (b) Serial output reachable, early boot | The T480 is an actively coreboot-supported mainboard with a documented real UART path via the embedded controller (EC UART), used specifically for early firmware/kernel debug console output in the coreboot project. This is the strongest, most concretely documented serial story of any modern (still-purchasable, still has NVMe + current-enough silicon) laptop researched this session — most current consumer/business laptops and mini PCs have no physical serial path at all. | [coreboot: Lenovo ThinkPad T470s/T480/T480s/T580/X280](https://doc.coreboot.org/mainboard/lenovo/skylake.html) |
| (c) NVMe or AHCI storage | Standard NVMe M.2 SSD slot; this is a well-known, unremarkable spec for this machine generation. | Widely documented ThinkPad T480 hardware maintenance manual / community teardown sources |
| (d) Publicly documented chipset | Intel 200-series (Union Point) PCH — publicly documented via Intel's own platform datasheets, the same chipset family this whole roadmap already assumes for Tier 1's `q35` target's real-world analog. | Intel public chipset documentation |

### Why this beat the alternatives researched this session

- **Generic modern mini PCs** (the obvious first instinct for a compact
  Tier 2 box) were the first thing searched — and the finding was
  negative and worth recording: practically no current mini PC ships
  with a physical serial port or a documented early-boot UART path.
  IOMMU/VT-d and NVMe are easy to satisfy on modern mini PCs; serial is
  the criterion that actually eliminates almost the whole category.
- **The classic OSDev-community "beige box" testing machine** (the
  OSDev Wiki's own long-standing testing-hardware advice) predates
  IOMMU as a requirement entirely and leans on legacy peripherals (floppy
  drives, ISA) that are irrelevant to this project's needs — not a good
  fit for a modern Rust/UEFI kernel with an IOMMU hard-gate.
- **Older ThinkPads** (X220/T420/T430-era, also well known in the
  firmware-hacking community for exposed UART headers) satisfy (b)
  arguably even more directly than the T480, but their CPU generations
  (Sandy/Ivy Bridge) are old enough that some specific SKUs' VT-d support
  needs the same per-unit verification, AND NVMe (criterion c) is not a
  native option on that generation (SATA AHCI only, pre-M.2-NVMe era) —
  the T480 satisfies all four criteria more cleanly at once.

## What's still required (not completable without physical hardware)

1. Acquire a real T480 unit (widely available secondhand — a common,
   inexpensive choice in the ThinkPad/coreboot hobbyist community).
2. Verify the exact CPU SKU installed has VT-d on ark.intel.com before
   relying on it (some base configurations may differ from the vPro
   SKUs assumed above).
3. Wire up the EC UART for serial output, following coreboot's own
   documented procedure for this mainboard.
4. Boot this project's actual UEFI bootloader + kernel on it and confirm
   the same `BOOT_START -> EXIT_BOOT_SERVICES_OK -> KERNEL_ENTER`
   checkpoint chain `scripts/test-boot.ps1` already asserts in QEMU,
   this time over the real serial line.

Only step 4 actually satisfies the roadmap's "brought to serial output"
wording — steps 1-3 are prerequisites for it, not substitutes.
