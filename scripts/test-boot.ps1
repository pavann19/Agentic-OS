# Runs the QEMU boot-test outside MSYS2 make's shell layer entirely.
#
# Root cause this works around: MSYS2's make.exe does not propagate TMP/TEMP
# (or any exported Makefile variable, confirmed by testing) into the real
# Win32 environment block of processes it spawns for recipes — `export` in
# a Makefile only sets a shell-local variable in make's recipe interpreter,
# never reaches child-process environ. QEMU's `-drive file=fat:rw:DIR`
# (vvfat) needs a writable TMP/TEMP to create scratch files and fails with
# "Could not open temporary file 'C:\...'" (falling back to the unwritable
# C:\ root) without it. Running this as a real PowerShell process sidesteps
# MSYS's environment layer altogether — $env:TMP here is genuinely part of
# this process's Win32 environment block, and children inherit it correctly.

# Real per-OS QEMU/OVMF defaults, resolved from the environment first
# (AGENTIC_OS_QEMU/AGENTIC_OS_OVMF_CODE, what CI sets), falling back to
# a bare command name resolvable via PATH (works out of the box after
# `apt-get install qemu-system-x86 ovmf` on Linux), falling back last to
# the winget-installed Windows path this repo has always defaulted to.
# The three-way fallback means neither platform needs a param override
# for the common case.
param(
    [string]$QemuExe = $(if ($env:AGENTIC_OS_QEMU) { $env:AGENTIC_OS_QEMU }
        elseif (Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue) { (Get-Command qemu-system-x86_64).Source }
        else { "C:\Program Files\qemu\qemu-system-x86_64.exe" }),
    [string]$OvmfCode = $(if ($env:AGENTIC_OS_OVMF_CODE) { $env:AGENTIC_OS_OVMF_CODE }
        elseif (Test-Path "/usr/share/OVMF/OVMF_CODE.fd") { "/usr/share/OVMF/OVMF_CODE.fd" }
        else { "C:\Program Files\qemu\share\edk2-x86_64-code.fd" }),
    [string]$FatDir = "boot_rs/qemu_fatdir",
    [string]$SerialLog = "_evidence/latest/serial.log",
    [string]$DiskImage = "_evidence/disk.img",
    [int]$TimeoutSeconds = 20
)

$ErrorActionPreference = "Stop"

# Self-locate the repo root from this script's own path rather than
# trusting the caller's current directory -- makes this script correct
# whether it's launched as `pwsh -File scripts/test-boot.ps1` from the
# repo root (every existing caller) or from anywhere else (CI's
# boot-test.sh, a future caller).
Set-Location (Join-Path $PSScriptRoot "..")

# ALWAYS overridden, never conditional on whether TMP/TEMP look already set:
# when this script is launched through `make` (MSYS2 make.exe spawns children
# with TMP/TEMP stripped, confirmed by testing), even
# .NET's own GetTempPath() fallback resolves to the unwritable C:\WINDOWS
# (its last-resort default once TMP/TEMP/USERPROFILE are all absent). A
# repo-local directory is the one thing this script can guarantee is both
# present and writable regardless of what launched it.
$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null

# Phase 4: a real, persistent-across-runs raw disk image for the virtio-blk
# device below. Created once (16MB, zero-filled) if it doesn't already
# exist -- deliberately NOT recreated every run, since a real Phase 4 exit
# criterion is "data written survives a reboot," which this same file
# staying around across consecutive test-boot.ps1 invocations is what lets
# a later increment actually prove.
New-Item -ItemType Directory -Force -Path (Split-Path $DiskImage) | Out-Null
if (-not (Test-Path $DiskImage)) {
    $fs = [System.IO.File]::Create($DiskImage)
    $fs.SetLength(16MB)
    $fs.Close()
}

# Start-Process -ArgumentList joins array elements with plain spaces — it
# does NOT auto-quote elements containing spaces (unlike ProcessStartInfo's
# newer ArgumentList property). $OvmfCode ("C:\Program Files\...") has a
# space in it, so it must be quoted manually here or qemu sees it split
# into two argv entries ("C:\Program" / "Files\...") and fails to find it.
$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    # Phase 4: real virtio-blk block device. disable-legacy=on forces the
    # modern-only PCI transport (the virtio_pci_cap capability-list layout
    # kernel_rs/src/pci.rs's find_virtio_caps parses) -- no legacy I/O-BAR
    # fallback to also support.
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
$exited = $proc.WaitForExit($TimeoutSeconds * 1000)
if (-not $exited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial log produced at $SerialLog"
    exit 1
}

$content = Get-Content $SerialLog -Raw
# REVOCATION_REJECTED_OK: the capability-revocation demo
# (kernel_rs/src/main.rs) that runs unconditionally on every default
# boot, not behind a feature flag -- a derived capability that worked
# a moment ago is used again immediately after its underlying object
# is revoked, and must be rejected on that exact same cap_id. Promoted
# to a required boot checkpoint here (previously demonstrated but
# never asserted by any script) since capability revocation is the
# central property this whole kernel is built around.
$required = @("BOOT_START", "EXIT_BOOT_SERVICES_OK", "KERNEL_ENTER", "REVOCATION_REJECTED_OK")
$missing = $required | Where-Object { $content -notmatch [regex]::Escape($_) }
if ($missing.Count -gt 0) {
    Write-Error "Missing checkpoint(s): $($missing -join ', ') in $SerialLog"
    Write-Output "--- serial.log ---"
    Write-Output $content
    exit 1
}

Write-Output "Checkpoints confirmed: $($required -join ' -> ')"
exit 0
