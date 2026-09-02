# Real power-loss-injection verification -- closes the honest gap
# Phase 4 disclosed as not separately demonstrated: "pulling power
# mid-write leaves the filesystem mountable -- corruption is bounded
# and detected, not silent" (`docs/ROADMAP.md` Phase 4 exit criteria).
#
# This is a REAL power-loss simulation, not a design argument: for
# several trials, each against a brand-new, never-formatted disk image,
# this script starts QEMU, lets `virtio_blk_driver` begin its real ext2
# format sequence, then SIGKILLs the whole QEMU process (Stop-Process
# -Force -- no ACPI shutdown, no flush, exactly what pulling the plug
# does to a real machine) at a point chosen to likely land mid-format,
# before the on-disk state is complete. The same, now-partially-written
# disk image is then booted again with NO reformatting forced -- this
# kernel decides for itself, from what's actually on disk, whether to
# treat it as formatted.
#
# What "bounded and detected, not silent" means here, concretely, and
# what this script actually checks for on the SECOND boot of each
# trial's disk:
#   - the kernel must not hang, crash, or double-fault reading a
#     half-written filesystem
#   - `FS_SELF_CHECK_FAIL` (a byte-mismatch on the read-back file) must
#     NEVER appear -- that would mean the system silently believed a
#     corrupt filesystem was good
#   - `FS_SELF_CHECK_PASS` MUST appear -- either because the interrupted
#     disk was correctly recognized as unformatted (superblock, written
#     LAST as the real commit point -- see virtio_blk_driver's
#     `run_filesystem_proof` -- was never reached) and safely
#     reformatted from scratch, or because the kill happened to land
#     after the real commit point and every prior structure was already
#     genuinely complete
#
# Same TMP/TEMP and QEMU-argument reasoning as test-boot.ps1 -- see that
# script's own comments, not repeated here.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$DiskImage = "_evidence\disk-powerloss-test.img",
    [int[]]$KillDelaysMs = @(1200, 1600, 2000, 2500, 3200, 5800, 7000),
    [int]$RecoveryWaitSeconds = 10
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $DiskImage) | Out-Null

function New-FreshDisk($path) {
    if (Test-Path $path) { Remove-Item -Force $path }
    $fs = [System.IO.File]::Create($path)
    $fs.SetLength(16MB)
    $fs.Close()
}

function Start-Qemu($diskPath, $serialLog) {
    $qemuArgs = @(
        "-machine", "q35,kernel-irqchip=split",
        "-m", "256M",
        "-device", "intel-iommu,intremap=on",
        "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
        "-drive", "file=fat:rw:$FatDir,format=raw",
        "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on",
        "-drive", "file=$diskPath,if=none,id=disk0,format=raw",
        "-serial", "file:$serialLog",
        "-display", "none",
        "-no-reboot"
    )
    Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
}

$allPassed = $true
$results = @()

for ($i = 0; $i -lt $KillDelaysMs.Length; $i++) {
    $delayMs = $KillDelaysMs[$i]
    $trialDisk = "_evidence\disk-powerloss-trial${i}.img"
    $log1 = "_evidence\powerloss-trial${i}-boot1.log"
    $log2 = "_evidence\powerloss-trial${i}-boot2.log"
    Remove-Item -Force $log1, $log2 -ErrorAction SilentlyContinue

    Write-Output "--- Trial ${i}: kill after ${delayMs}ms (simulated power loss mid-format) ---"
    New-FreshDisk $trialDisk

    # Boot 1: real power loss -- SIGKILL, no clean shutdown, no flush.
    $proc1 = Start-Qemu $trialDisk $log1
    Start-Sleep -Milliseconds $delayMs
    if (-not $proc1.HasExited) {
        Stop-Process -Id $proc1.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 300 # let the OS actually release the disk file handle

    # Boot 2: same, now partially-written disk image, real recovery.
    $proc2 = Start-Qemu $trialDisk $log2
    try {
        Start-Sleep -Seconds $RecoveryWaitSeconds
    } finally {
        if (-not $proc2.HasExited) {
            Stop-Process -Id $proc2.Id -Force -ErrorAction SilentlyContinue
        }
    }

    if (-not (Test-Path $log2)) {
        Write-Output "  FAIL: no serial output at all on recovery boot"
        $allPassed = $false
        $results += "trial ${i} (${delayMs}ms): FAIL -- no recovery boot output"
        continue
    }

    $content2 = Get-Content $log2 -Raw
    $sawFail = $content2 -match [regex]::Escape("FS_SELF_CHECK_FAIL")
    $sawPass = $content2 -match [regex]::Escape("FS_SELF_CHECK_PASS")
    $sawReformat = $content2 -match [regex]::Escape("FS_NOT_FORMATTED")
    $sawAlready = $content2 -match [regex]::Escape("FS_ALREADY_FORMATTED")

    if ($sawFail) {
        Write-Output "  FAIL: FS_SELF_CHECK_FAIL observed -- silent corruption, not bounded/detected"
        $allPassed = $false
        $results += "trial ${i} (${delayMs}ms): FAIL -- silent corruption observed"
    } elseif (-not $sawPass) {
        Write-Output "  FAIL: recovery boot never reached FS_SELF_CHECK_PASS (hang or crash reading the interrupted disk)"
        $allPassed = $false
        $results += "trial ${i} (${delayMs}ms): FAIL -- no self-check pass, no explicit fail either (hang/crash)"
    } else {
        $how = if ($sawReformat) { "detected as unformatted, safely reformatted" } elseif ($sawAlready) { "commit point (superblock) had already landed, no reformat needed" } else { "self-check passed" }
        Write-Output "  PASS: filesystem recovered cleanly -- $how"
        $results += "trial ${i} (${delayMs}ms): PASS -- $how"
    }
}

Write-Output ""
Write-Output "=== Power-loss injection summary ==="
$results | ForEach-Object { Write-Output "  $_" }

if (-not $allPassed) {
    Write-Error "One or more power-loss trials showed unbounded/undetected corruption."
    exit 1
}

Write-Output ""
Write-Output "All power-loss trials recovered cleanly: interrupted formats are either completed to a consistent state or safely detected as unformatted and reformatted -- never silently trusted as good when they were not."
exit 0
