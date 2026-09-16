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
| Capability table, generation-based revocation, attenuation | Verified in CI | `scripts/ci/boot-test.sh revoke` (asserts `REVOCATION_REJECTED_OK`) + `host_tests` | Passing |
| Syscall boundary / IPC | Verified in CI | Exercised by every check above (all of them cross this boundary) | Passing |
| Paging / address-space isolation | Verified in CI | Implicit in every successful boot (ring-3 processes only run with working isolated address spaces) + `host_tests` | Passing |
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

## Keeping this honest

A row moves from Demonstrated to Verified in CI when a real CI job
exercises it — not when it's demonstrated again by hand. Update this
table in the same commit as whatever changes a row's status.
