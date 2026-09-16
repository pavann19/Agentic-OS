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
TIMEOUT_SECONDS=25

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
timeout --signal=TERM "${TIMEOUT_SECONDS}s" "$QEMU" \
    -machine q35,kernel-irqchip=split \
    -m 256M \
    -device intel-iommu,intremap=on \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive file=fat:rw:"$FAT_DIR",format=raw \
    -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on \
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
