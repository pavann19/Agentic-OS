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
| Capability table, generation-based revocation, attenuation | Verified in CI | `scripts/ci/boot-test.sh revoke` (asserts `REVOCATION_REJECTED_OK`) + `host_tests` | Passing — 3 consecutive green CI runs watched directly before this row was flipped back (see Known Issues for the root cause and fix). |
| Syscall boundary / IPC | Verified in CI | Exercised by every check above (all of them cross this boundary) | Passing |
| Paging / address-space isolation | Verified in CI | Implicit in every successful boot (ring-3 processes only run with working isolated address spaces) + `host_tests` | Passing |
| UEFI boot -> kernel handoff | Verified in CI | `scripts/ci/boot-test.sh boot` | Passing |
| One supervised user-space driver (virtio-blk) | Demonstrated | `scripts/test-boot.ps1` (Windows), `scripts/test-faults.ps1` for crash/restart | Passing locally; not yet in CI |
| Syscall round-trip latency (the plan's "one measured systems metric") | Verified in CI | `scripts/ci/boot-test.sh revoke` (asserts a real `SYSCALL_LATENCY` marker) + `scripts/test-syscall-latency.ps1` (Windows, real numeric bounds, not just presence) | Passing — see `docs/PERFORMANCE_BASELINE.md`'s own section for the numbers and the honest QEMU/TCG-not-hardware caveat |

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

## Resolved issues

### CI-only, non-deterministic page fault during the `revoke` boot run

Resolved. `scripts/ci/boot-test.sh revoke` used to reproducibly crash
the kernel before reaching `REVOCATION_REJECTED_OK`, on GitHub
Actions' Ubuntu runner only — never once reproduced across this
entire project's local testing (Windows, WHPX and TCG both).

The investigation ruled out several concrete hypotheses in turn, each
checked against real evidence rather than guessed:
- Bootloader/kernel page-mapping math (`boot_rs/src/loader.rs`,
  `kernel_rs/src/vmm.rs`) — both use correct ceiling division.
- An acquire/release imbalance in the kernel's single coarse-grained
  lock (`kernel_rs/src/critical.rs`) — instrumented it directly, the
  lock state at fault time was always balanced.
- An interrupt landing mid-handshake during IOMMU register access —
  masked interrupts across the whole sequence, crash persisted.
- A missing hardware EOI in the LAPIC timer ISR (`kernel_rs/src/idt.rs`'s
  `h_timer`) — this WAS a real, independent bug (found and fixed
  regardless of whether it was the root cause here: `h_timer` never
  called `apic::eoi()`, unlike the other two LAPIC-sourced handlers) —
  but fixing it alone didn't clear the hang either.
- A page-table index collision between the AP trampoline's low
  identity-mapped address and high kernel virtual addresses — checked
  the actual index math (`kernel_common::pagetable::split_indices`);
  the two live in different PML4 slots entirely, no possible overlap.

Root cause: every crash traced back to `smp::bring_up_all`'s
trampoline-copy-and-identity-map setup, which ran unconditionally even
though the MADT on this CI runner lists only the BSP (no real APs to
ever bring up) — the function's own per-CPU loop does nothing in that
case. Fixed by skipping the whole call when the MADT reports no real
APs (`kernel_rs/src/main.rs`), removing the exact code region every
crash pointed at. Also simplified CI's own QEMU invocation along the
way (`scripts/ci/boot-test.sh`): dropped the `intel-iommu` device and
`kernel-irqchip=split` (neither is needed for what this check
verifies, and both were red herrings chased during the investigation),
and raised the timeout to a more realistic 60s.

Verified: 3 consecutive green CI runs watched directly (not assumed)
before this section was written and `docs/VERIFICATION.md`'s
revocation row was flipped back to Verified in CI.

### The missing "one measured systems metric"

Resolved. The original CI/evidence plan asked for one real measured systems
metric alongside CI, boot, revocation, fuzzing, `docs/DECISIONS.md`, and the
demo video — everything else in that plan landed in the same session; this
one was overlooked and had no doc or `_evidence` file backing it, caught
later by re-reading the original plan against what had actually shipped.

Added as real syscall round-trip latency, measured with `RDTSC` from inside
the guest (`user_rs/serial_driver`, the earliest real ring-3 ELF spawned on
every boot) rather than host-side wall-clock polling, which is far too
coarse at this scale. Two real bugs surfaced getting it working at all: a
2000-sample stack array overran the process's single 4KB ring-3 stack page
(process silently killed, page fault), and the array's own zero-init/build
step twice triggered a pre-existing toolchain bug (`kernel_common::
mem_intrinsics`'s own module doc: LLVM's `memset`/`memcpy` lowering becomes
an indirect call through an unpopulated import-style slot on this host
toolchain) even with that module's existing fix linked in — worked around
with a `MaybeUninit` array filled by scalar stores only, never a bulk
operation. See `docs/PERFORMANCE_BASELINE.md`'s own section for the real
numbers and the honest QEMU/TCG framing.

## Keeping this honest

A row moves from Demonstrated to Verified in CI when a real CI job
exercises it — not when it's demonstrated again by hand. Update this
table in the same commit as whatever changes a row's status.
