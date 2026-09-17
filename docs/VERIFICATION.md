# Verification Matrix

What is actually machine-checked, in CI, on every push — versus what is
real, working code that has only been demonstrated by hand. This page
replaces `docs/FEATURE_STATUS.md`'s table (kept as a thin pointer here)
with the same information in CI's own vocabulary.

Status legend:
- **Verified in CI** — `.github/workflows/ci.yml` builds and boots this
  and asserts a real marker for it, on every push/PR, on Linux.
- **Demonstrated** — real, working code with a local test script
  (`scripts/test-*.ps1`, run manually / as part of the 26-suite
  Windows regression battery) that isn't wired into CI yet.
- **Partial** — a named, disclosed gap in either the implementation or
  its test coverage (see the Notes column).
- **Experimental** — runs as part of a normal boot, but nothing
  anywhere asserts its output; a regression here is currently silent.

Everything below runs under QEMU/TCG software emulation. Nothing has
been run on physical hardware yet (`docs/ROADMAP.md` §4, Tier 2/3) —
that gap applies uniformly and isn't repeated per row.

## Core defended scope

The subsystems the security model actually depends on — capability
table, the boundary that enforces it, and the isolation guarantees
that make the boundary meaningful. This is the scope CI actually
proves; everything past this section is real but not (yet) part of
that proof.

| Subsystem | Status | Test | Last result |
|---|---|---|---|
| Capability table, generation-based revocation, attenuation | Partial | `scripts/ci/boot-test.sh revoke` (asserts `REVOCATION_REJECTED_OK`) + `host_tests` | **Not currently passing in CI.** A real, reproducible-but-nondeterministic kernel page fault (different exact instruction each time; see notes below) crashes this specific boot run before the marker is reached, on this CI runner only -- never reproduced locally. `host_tests` (the pure-logic capability/revocation unit tests) still pass; the live-boot assertion does not. Do not flip this back to "Verified in CI" until a real CI run has been watched going green. |
| Syscall boundary / IPC | Demonstrated | Exercised by every check above (all of them cross this boundary) | The `boot` check (not `revoke`) passes reliably; treating this as fully "Verified in CI" would overstate it while `revoke` is red. |
| Paging / address-space isolation | Partial | Implicit in every successful boot (ring-3 processes only run with working isolated address spaces) + `host_tests` | The open `revoke`-run crash is itself a paging-related fault (a legitimate, already-mapped instruction's page reads not-present at the moment of the fault) -- ironic but real: this row cannot honestly claim "Verified" while investigating a live paging bug. |
| UEFI boot -> kernel handoff | Verified in CI | `scripts/ci/boot-test.sh boot` | Passing |
| One supervised user-space driver (virtio-blk) | Demonstrated | `scripts/test-boot.ps1` (Windows), `scripts/test-faults.ps1` for crash/restart | Passing locally; not yet in CI |

## Everything else

Real, but Partial or Experimental in this matrix until its own test is
wired into CI — per the defended-scope decision, adding to this table
requires an actual CI job, not just an assertion here.

| Subsystem | Status | Test | Notes |
|---|---|---|---|
| Fault isolation / driver crash-restart (general) | Demonstrated | `test-faults` | |
| Multi-core (SMP) bring-up | Partial | `test-boot` (bring-up only) | Race-soak skips itself below 3 real cores; not exercised in CI. |
| Power-loss / crash safety (ext2) | Demonstrated | `test-powerloss` | Superblock-last write ordering; does not claim mid-superblock-write safety (disclosed in `virtio_blk_driver`'s own doc). |
| Storage: AHCI, NVMe | Demonstrated | `test-ahci`, `test-nvme` | |
| Filesystem: ext2 directories/path resolution | Demonstrated | `test-self-hosting` | Parsers also covered by `kernel_common/fuzz/` (bounded smoke-fuzz in CI, not exhaustive). |
| USB: xHCI | Demonstrated | `test-xhci` | |
| USB HID (mouse/keyboard) | Partial | none | Configure Endpoint bug still open (Phase 11); PS/2 is the only tested input path. |
| Network: e1000, virtio-net/TCP | Demonstrated | `test-e1000`, `test-synthesis`, `test-tcp-two-instance` | |
| Display server / compositor | Demonstrated | `test-compositor`, `test-compositor-crash` | |
| Input routing (PS/2 -> focused window) | Demonstrated | `test-input-routing` | Fixed this session (false-positive `FocusWindow`/silent `InjectKey` drop). |
| Reference apps, manifest, installer, shell | Demonstrated | `test-terminal`, `test-text-editor`, `test-file-manager`, `test-manifest`, `test-installer`, `test-shell` | |
| Live Agent Bridge (COM2 external-agent control) | Demonstrated | `test-agent-bridge` | Also driven live, ad hoc, over the real protocol this session. |
| Agent-driven internet download | Demonstrated | `test-agent-download` | |
| Self-hosting (spawn/wait/exit, path-based exec) | Demonstrated | `test-self-hosting` | `Rights::EXEC`-gated; adversarial denial case included. |
| Multi-user / update integrity / TPM root of trust | Partial | none in regression battery | Demonstrated at landing time; not re-run automatically since. |
| Agent runtime substrate (Phase 5 capability demo) | Experimental | none | Runs every boot; nothing asserts its output. |
| Release build reproducibility | Demonstrated | `test-release` | |
| AMD chipset support | Partial | none | Every driver/IOMMU path validated only against QEMU's Intel-chipset-modeled `q35` + Intel-vendor virtual PCI IDs. |
| Real hardware (Tier 2/3) | Partial | none | The single largest gap — see `docs/ROADMAP.md` §4. |

## Known issues

### CI-only, non-deterministic page fault during the `revoke` boot run

Open. `scripts/ci/boot-test.sh revoke` reproducibly crashes the kernel
before reaching `REVOCATION_REJECTED_OK`, on GitHub Actions' Ubuntu
runner only — never once reproduced across this entire project's local
testing (Windows, WHPX and TCG both). `boot-test.sh boot` (identical
QEMU invocation, only the asserted markers differ) passes reliably.

What's been ruled out, by reading the actual code, not by guessing:
- The bootloader's PT_LOAD page-count math (`boot_rs/src/loader.rs`)
  and the kernel's own per-segment page-mapping loop
  (`kernel_rs/src/vmm.rs`) both use correct ceiling division — no
  off-by-one there.
- The crash address each time falls inside the linker's declared
  `[__text_start, __text_end)` executable range and disassembles to
  real, valid, already-present code in the actual booted binary
  (confirmed by downloading CI's own `kernel-elf` artifact and
  resolving the crash address against it directly — a locally-built
  binary is NOT reliable for this, since this session confirmed
  cross-host toolchain builds can lay out code differently even from
  identical source).

What's confirmed: the exact faulting instruction differs between runs
(`idt::recover_or_halt`'s own address in one run;
`critical::release`'s lock-owner-clear/jump in another) — this is a
genuine timing-dependent race, not a fixed bug at one address. Both
observed sites are plausibly connected to interrupt timing around
early boot's ACPI/IOMMU/SMP bring-up sequence and/or the kernel's
single coarse-grained lock (`kernel_rs/src/critical.rs`) — an
unbalanced acquire/release if a fault interrupts code between
`acquire()` succeeding and `release()` running is one live hypothesis,
not yet confirmed.

Diagnostic instrumentation (`IOMMU_DIAG` markers,
`vmm::debug_translate` page-table reads at each IOMMU-init sub-step)
is committed in `kernel_rs/src/iommu.rs` but has not yet caught the
fault in the act — the crash has landed before `iommu::init()` starts
as often as during it. Real next steps: instrument
`kernel_rs/src/critical.rs`'s acquire/release pair for imbalance
detection, and/or bisect by disabling early-boot subsystems
(ACPI/SMP/IOMMU) one at a time in a CI-only build to localize which
one's timing is implicated.

## Keeping this honest

A row moves from Demonstrated to Verified in CI when a real CI job
exercises it — not when it's demonstrated again by hand. Update this
table in the same commit as whatever changes a row's status.
