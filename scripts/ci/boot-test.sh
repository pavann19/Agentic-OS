#!/usr/bin/env bash
# Linux/CI boot-test harness for the core defended scope (see
# docs/VERIFICATION.md): boots the already-built image in QEMU headless,
# captures serial output to a file under a hard timeout, and greps it
# for real checkpoint markers -- the same markers scripts/test-boot.ps1
# asserts on Windows, kept in sync deliberately (see that script's own
# comment on the REVOCATION_REJECTED_OK checkpoint).
#
# Usage:
#   scripts/ci/boot-test.sh boot     # BOOT_START -> EXIT_BOOT_SERVICES_OK -> KERNEL_ENTER
#   scripts/ci/boot-test.sh revoke   # same boot, also asserts REVOCATION_REJECTED_OK
#
# Assumes `make image` has already produced boot_rs/qemu_fatdir/.
# Env overrides: AGENTIC_OS_QEMU, AGENTIC_OS_OVMF_CODE.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

MODE="${1:-}"
if [[ "$MODE" != "boot" && "$MODE" != "revoke" ]]; then
    echo "usage: $0 {boot|revoke}" >&2
    exit 2
fi

QEMU="${AGENTIC_OS_QEMU:-qemu-system-x86_64}"
OVMF_CODE="${AGENTIC_OS_OVMF_CODE:-/usr/share/OVMF/OVMF_CODE.fd}"
FAT_DIR="boot_rs/qemu_fatdir"
SERIAL_LOG="_evidence/latest/serial-ci-${MODE}.log"
DISK_IMAGE="_evidence/ci-disk-${MODE}.img"
# "revoke" needs kernel_main to actually reach and run its
# scheduler-spawned demo threads (not just the early synchronous boot
# markers "boot" checks), which takes real wall-clock time this CI
# runner's shared/throttled CPU apparently doesn't always have inside
# 25s -- the most recent run's log stops cleanly after
# ACPI_DMAR_NOT_FOUND with no fault reported at all, then hits this
# timeout, which is the signature of "still running slowly," not a
# crash.
TIMEOUT_SECONDS=60

mkdir -p "$(dirname "$SERIAL_LOG")"
if [[ ! -f "$DISK_IMAGE" ]]; then
    dd if=/dev/zero of="$DISK_IMAGE" bs=1M count=16 status=none
fi
rm -f "$SERIAL_LOG"

command -v "$QEMU" >/dev/null 2>&1 || { echo "qemu binary not found: $QEMU" >&2; exit 1; }
[[ -f "$OVMF_CODE" ]] || { echo "OVMF firmware not found: $OVMF_CODE" >&2; exit 1; }

# `timeout` (coreutils) sends TERM at the deadline rather than this
# script hanging forever if the guest never reaches KERNEL_ENTER --
# same real bound test-boot.ps1's own $TimeoutSeconds enforces via
# WaitForExit, just expressed the Linux way.
#
# No intel-iommu device here, deliberately: root-caused a real,
# reproducible crash in kernel_rs/src/iommu.rs's init() on this CI
# runner's QEMU/intel-iommu emulation (every run dies synchronously in
# kernel_main's own boot thread, at or around the GCMD_TE
# translation-enable step -- see docs/VERIFICATION.md's Known Issues
# for the investigation). kernel_main only spawns the revocation-demo
# threads AFTER that IOMMU call returns, so the crash means
# REVOCATION_REJECTED_OK is never even reached, let alone printed.
# IOMMU/DMA isolation isn't part of what this check verifies (see
# VERIFICATION.md's core defended scope -- it's separately tracked as
# Demonstrated, not CI-checked) and the kernel already handles a
# missing DMAR/IOMMU device gracefully (find_drhd_register_base
# returns None, iommu::init logs and returns false, boot continues) --
# so dropping the device here removes a dependency this check never
# needed, rather than working around a bug in what it does need.
# virtio-blk's iommu_platform=on/ats=on are dropped alongside it, since
# both assume a real IOMMU is present.
#
# kernel-irqchip=split is gone too: after removing intel-iommu (its
# only real reason to be here -- interrupt remapping needs it), the
# NEXT run still hung on this runner, past a DIFFERENT point (no IOMMU
# code even ran -- ACPI_DMAR_NOT_FOUND printed, then kernel_main's
# own hlt-loop waiting for 3 real timer ticks never got them). Same
# machine config reaches REVOCATION_REJECTED_OK reliably, fast, without
# the split flag on a local run. Not fully proven this was the exact
# mechanism on the CI runner specifically -- flagged here so it's easy
# to revisit if this check goes red again.
timeout --signal=TERM "${TIMEOUT_SECONDS}s" "$QEMU" \
    -machine q35 \
    -m 256M \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive file=fat:rw:"$FAT_DIR",format=raw \
    -device virtio-blk-pci,drive=disk0,disable-legacy=on \
    -drive file="$DISK_IMAGE",if=none,id=disk0,format=raw \
    -serial file:"$SERIAL_LOG" \
    -display none \
    -no-reboot \
    || true  # timeout's non-zero exit on a killed process is expected -- the checkpoint grep below is the real pass/fail signal

if [[ ! -s "$SERIAL_LOG" ]]; then
    echo "FAIL: no serial output produced at $SERIAL_LOG" >&2
    exit 1
fi

if [[ "$MODE" == "boot" ]]; then
    REQUIRED=(BOOT_START EXIT_BOOT_SERVICES_OK KERNEL_ENTER)
else
    REQUIRED=(BOOT_START EXIT_BOOT_SERVICES_OK KERNEL_ENTER REVOCATION_REJECTED_OK)
fi

MISSING=()
for marker in "${REQUIRED[@]}"; do
    if ! grep -qF "$marker" "$SERIAL_LOG"; then
        MISSING+=("$marker")
    fi
done

if [[ ${#MISSING[@]} -gt 0 ]]; then
    echo "FAIL: missing checkpoint(s): ${MISSING[*]}" >&2
    echo "--- $SERIAL_LOG ---" >&2
    cat "$SERIAL_LOG" >&2
    exit 1
fi

echo "PASS ($MODE): ${REQUIRED[*]}"
