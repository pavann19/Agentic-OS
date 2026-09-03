# Phase 8, deliverable 5: real Tier 1 (QEMU) boot-time and I/O
# measurements, captured as a tracked regression baseline
# (docs/PERFORMANCE_BASELINE.md). Real host-side wall-clock timing
# around real checkpoint markers appearing in the serial log --
# genuinely measured on THIS run, not simulated or copied from a
# prior session's numbers.
#
# Honest scope: these are Tier 1 (QEMU/TCG) numbers. They say nothing
# about real Tier 2 hardware performance -- QEMU's software emulation
# has a completely different performance profile than real silicon
# (see docs/SUPPORTED_HARDWARE.md). Their value here is as a
# REGRESSION baseline (did a change make Tier 1 boot meaningfully
# slower), not an absolute performance claim about the eventual
# hardware target.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [int]$Runs = 3
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

function Measure-BootToMarker($marker, $diskPath) {
    if (Test-Path $diskPath) { Remove-Item -Force $diskPath }
    $fs = [System.IO.File]::Create($diskPath)
    $fs.SetLength(16MB)
    $fs.Close()

    $log = "_evidence\perf-measure.log"
    Remove-Item -Force $log -ErrorAction SilentlyContinue

    $qemuArgs = @(
        "-machine", "q35,kernel-irqchip=split", "-m", "256M",
        "-device", "intel-iommu,intremap=on",
        "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
        "-drive", "file=fat:rw:$FatDir,format=raw",
        "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
        "-drive", "file=$diskPath,if=none,id=disk0,format=raw",
        "-serial", "file:$log", "-display", "none", "-no-reboot"
    )

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
    $found = $false
    for ($i = 0; $i -lt 300; $i++) {
        Start-Sleep -Milliseconds 100
        if (Test-Path $log) {
            $c = Get-Content $log -Raw -ErrorAction SilentlyContinue
            if ($c -match [regex]::Escape($marker)) { $found = $true; break }
        }
    }
    $sw.Stop()
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    Start-Sleep -Milliseconds 300
    if (-not $found) { return $null }
    return $sw.ElapsedMilliseconds
}

Write-Output "=== Measuring: boot -> KERNEL_ENTER (real, $Runs runs) ==="
$kernelEnterTimes = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $ms = Measure-BootToMarker "KERNEL_ENTER" "_evidence\perf-disk-a.img"
    if ($null -ne $ms) {
        Write-Output "  run $($i+1): ${ms}ms"
        $kernelEnterTimes += $ms
    }
}

Write-Output ""
Write-Output "=== Measuring: boot -> full steady state (FS_SELF_CHECK_PASS, fresh-format ext2 + virtio-blk I/O, real, $Runs runs) ==="
$fsCheckTimes = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $ms = Measure-BootToMarker "FS_SELF_CHECK_PASS" "_evidence\perf-disk-b.img"
    if ($null -ne $ms) {
        Write-Output "  run $($i+1): ${ms}ms"
        $fsCheckTimes += $ms
    }
}

function Median($arr) {
    $sorted = $arr | Sort-Object
    $n = $sorted.Count
    if ($n -eq 0) { return $null }
    if ($n % 2 -eq 1) { return $sorted[[math]::Floor($n / 2)] }
    return [math]::Round(($sorted[$n / 2 - 1] + $sorted[$n / 2]) / 2.0, 1)
}

$result = [ordered]@{
    measured_at_utc         = (Get-Date).ToUniversalTime().ToString("o")
    platform                = "Tier 1 (QEMU q35 + OVMF + TCG software emulation) -- NOT real hardware, see docs/SUPPORTED_HARDWARE.md"
    boot_to_kernel_enter_ms = [ordered]@{ runs = $kernelEnterTimes; median = (Median $kernelEnterTimes) }
    boot_to_fs_selfcheck_ms = [ordered]@{ runs = $fsCheckTimes; median = (Median $fsCheckTimes) }
}

Write-Output ""
Write-Output "=== Summary (medians) ==="
Write-Output "  boot -> KERNEL_ENTER:        $($result.boot_to_kernel_enter_ms.median) ms"
Write-Output "  boot -> FS_SELF_CHECK_PASS:  $($result.boot_to_fs_selfcheck_ms.median) ms"

$result | ConvertTo-Json -Depth 5 | Set-Content "_evidence\perf-result.json"
Write-Output ""
Write-Output "Raw result: _evidence\perf-result.json"

Remove-Item -Force "_evidence\perf-disk-a.img", "_evidence\perf-disk-b.img", "_evidence\perf-measure.log" -ErrorAction SilentlyContinue
