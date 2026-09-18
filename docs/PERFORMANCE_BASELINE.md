# Performance Baseline (Tier 1 / QEMU)

Phase 8, deliverable 5 (`docs/ROADMAP.md` §5): "Boot-time and I/O throughput measurements captured as tracked regression baselines." Real numbers, measured by `scripts/measure-performance.ps1` against this project's own Tier 1 target (QEMU `q35` + OVMF, TCG software emulation), not simulated or estimated.

**Scope, stated honestly:** these are Tier 1 numbers only. QEMU's TCG software emulation has a completely different performance profile than real silicon — nothing here predicts real Tier 2 hardware performance (see `docs/SUPPORTED_HARDWARE.md`). Their purpose is **regression tracking**: re-run `scripts/measure-performance.ps1` after a change and compare against the figures below — a meaningful slowdown is worth investigating even though the absolute numbers are QEMU-specific. Real Tier 2 hardware baselines are separate, future work, gated on physically owning the hardware (see `docs/PROGRESS.md`'s Phase 3/8 notes).

Host machine variance is real and not controlled for here (this ran on whatever machine this development session's own host was) — treat the numbers as this specific host's own reference point, re-baseline on a new host before trusting cross-host comparisons.

## Boot time

| Measurement | Median (3 runs) | Individual runs | What it covers |
|---|---|---|---|
| Boot start → `KERNEL_ENTER` | **4407 ms** | 4538, 4325, 4407 ms | UEFI firmware + `boot_rs`'s own bootloader (ELF load, memory map, `ExitBootServices`) through the kernel's own entry point |
| Boot start → `FS_SELF_CHECK_PASS` | **5076 ms** | 5188, 5076, 4951 ms | Full steady-state boot: the above, plus PMM/VMM/heap/timer init, PCI enumeration, IOMMU bring-up, device manager, `init`/service manager, all Phase 3-7 driver spawns (serial, framebuffer, keyboard, `virtio-blk`, `virtio-net`, the three Phase 5 agent demos, the Phase 7 shell), AND a real `virtio-blk` I/O round trip — a fresh ext2 format (superblock through file data, block-by-block) plus a full file read-back and byte comparison |

Measured 2026-09-03, this development session's own host machine. Note the two figures are close (~670ms apart) despite the second covering far more real work — almost all of this kernel's own boot sequence executes fast relative to the ~4.4s UEFI-firmware-and-bootloader phase that precedes it on this host under QEMU/TCG.

## I/O

Real `virtio-blk` throughput and `virtio-net` round-trip latency are natural next additions to this baseline — not yet separately measured/tracked as their own numbers (the boot-time figure above already exercises real `virtio-blk` I/O as part of the ext2 self-check, but doesn't isolate a throughput number from it). Real future work, not glossed over as already covered.

## Syscall round-trip latency

The "one measured systems metric" the original CI/evidence plan called for — landed later than everything else in that plan (CI, boot, revocation, fuzzing, `docs/DECISIONS.md`, the demo video), added here once CI was stable enough to be worth extending.

Measured from inside the guest with `RDTSC`, not host wall-clock timing (host-side polling is far too coarse for a syscall-scale measurement) — `user_rs/serial_driver` times 63 round trips of syscall 1 (`SYSCALL_LOG`, a real kernel-side COM1 PIO write, not a synthetic no-op) immediately after it starts, the earliest real ring-3 ELF spawned on every boot, feature-flag or not. See that file's own comments for two real bugs found getting this measurement to run at all: a 2000-sample array blew through the process's single 4KB ring-3 stack page, and a naive `[0u64; N]`/`core::array::from_fn` array build both triggered a pre-existing toolchain bug (`kernel_common::mem_intrinsics`'s own module doc) where LLVM's `memset`/`memcpy` lowering becomes an indirect call through an unpopulated slot on this host toolchain — worked around with a `MaybeUninit` array filled by scalar stores only, never a bulk zero-init or bulk copy.

| Measurement | Value (one representative run) | Notes |
|---|---|---|
| Median | 588,060 cycles (~246 µs, nominal 2.5GHz estimate) | Real, sorted median of 63 samples |
| Min | 344,883 cycles (~140 µs) | Best case observed |
| Max | 529,470,549 cycles | A real, expected outlier — a timer/IRQ landing between the two `RDTSC` reads of that one sample, not a bug; median is reported specifically because it's robust to this |

**Read this honestly:** these are QEMU/TCG per-instruction-interpretation numbers, not a hardware syscall cost — real hardware SYSCALL/SYSRET round trips are typically tens to low hundreds of nanoseconds, one to three orders of magnitude faster than what's measured here. The microsecond estimate above uses the same unverified nominal ~2.5GHz constant `kernel_rs::compositor_metrics::CYCLES_PER_US` already uses elsewhere in this codebase — a readability aid, not a calibrated wall-clock number. The **cycle counts are the real, exact numbers** this run produced; treat them as a same-platform regression baseline (did a change make dispatch meaningfully slower or more variable), same as every other number on this page.

Re-measured automatically by `scripts/test-syscall-latency.ps1` (wired into `scripts/test-integration.ps1`) and asserted (presence + real bound checks, not just presence) in CI's own `revoke` boot check (`scripts/ci/boot-test.sh`) — this is the one metric on this page that IS machine-checked on every push, not just locally re-measured on demand.

## How to re-measure

```powershell
powershell -ExecutionPolicy Bypass -File scripts/measure-performance.ps1
```

Raw JSON output (every individual run, not just the median) is written to `_evidence\perf-result.json` for closer inspection.
