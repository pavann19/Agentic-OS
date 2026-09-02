# Phase 8, deliverable 3: `test-release` -- boots the actual RELEASE
# artifacts (scripts/build-release.ps1's own output, a real clean
# build), not the dev fatdir `test-boot.ps1` uses, and asserts the same
# real checkpoint chain. This is what "the release image genuinely
# boots" means as evidence, not an assumption that a release build
# behaves like the dev build that's been iterated on all along.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$ReleaseDir = "dist\agentic-os-release",
    [string]$SerialLog = "_evidence\latest\serial-release.log",
    [string]$DiskImage = "_evidence\disk-release-test.img",
    [int]$BootWaitSeconds = 8
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path "$ReleaseDir\MANIFEST.json")) {
    Write-Output "No release build found at $ReleaseDir -- building one first."
    powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -OutDir $ReleaseDir
    if ($LASTEXITCODE -ne 0) { throw "release build failed" }
}

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path $DiskImage) | Out-Null
if (-not (Test-Path $DiskImage)) {
    $fs = [System.IO.File]::Create($DiskImage)
    $fs.SetLength(16MB)
    $fs.Close()
}

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$ReleaseDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

Remove-Item -Force $SerialLog -ErrorAction SilentlyContinue
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output from the release build boot"
    exit 1
}

$content = Get-Content $SerialLog -Raw
$checkpoints = @("BOOT_START", "EXIT_BOOT_SERVICES_OK", "KERNEL_ENTER")
$allFound = $true
foreach ($cp in $checkpoints) {
    if ($content -match [regex]::Escape($cp)) {
        Write-Output "  PASS: $cp"
    } else {
        Write-Output "  FAIL: $cp not found"
        $allFound = $false
    }
}

$manifest = Get-Content "$ReleaseDir\MANIFEST.json" | ConvertFrom-Json
Write-Output ""
Write-Output "Release artifacts booted (built $($manifest.built_at_utc)):"
$manifest.artifacts.PSObject.Properties | ForEach-Object { Write-Output "  $($_.Name): $($_.Value)" }

if ($allFound) {
    Write-Output ""
    Write-Output "Release image boots correctly: real checkpoint chain confirmed from the actual release artifacts, not the dev build."
    exit 0
} else {
    Write-Error "Release image failed to boot correctly -- see checkpoints above."
    exit 1
}
