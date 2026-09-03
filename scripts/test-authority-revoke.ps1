# Research track (docs/RESEARCH_TRACK.md, docs/NOVEL_CONCEPTS.md
# sections 1, 2, and 3): real-hardware verification of the full
# authority-graph-to-hardware wiring (kernel_rs/src/authority.rs,
# kernel_rs/src/authority_hw_fault_demo.rs) -- section 1's device
# (IOMMU context-table) AND CPU (page-table) sides, section 2's
# hardware-bound impossibility certificate with a real byte-level
# corruption test, and section 3's least-privilege envelope discovered
# from REAL captured IOMMU fault addresses and frozen into real,
# enforced grants. Builds the kernel WITH the
# `research_authority_hw_demo` feature (off by default -- see
# Cargo.toml's own comment on why: several of these demos' deliberate
# bounded waits for expected-to-fail commands add real wall-clock delay
# unsuitable for every normal boot), boots it against a real AHCI disk
# under a real `intel-iommu` device, and asserts every real PASS marker
# fired. Rebuilds the default (non-feature) kernel afterward, same
# discipline as scripts/test-faults.ps1, so the tree is left in its
# normal state, not mid-research-build.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-authority-revoke.log",
    [string]$DiskImage = "_evidence\disk-authority-revoke-test.img",
    [string]$AhciDiskImage = "_evidence\disk-authority-revoke-ahci.img",
    [int]$BootWaitSeconds = 20
)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
# Set-Location only changes PowerShell's own $PWD -- .NET APIs like
# [System.IO.File]::Create resolve relative paths against
# [Environment]::CurrentDirectory instead, which does NOT follow
# Set-Location automatically. Found by this script's own first real
# run failing with a path rooted at the wrong directory -- synced
# explicitly here rather than assumed.
[Environment]::CurrentDirectory = $repoRoot

function Invoke-CargoQuiet {
    param([string[]]$CargoArgs)
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo @CargoArgs 2>&1 | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $saved
    }
}

$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
foreach ($d in @($DiskImage, $AhciDiskImage)) {
    if (-not (Test-Path $d)) {
        $fs = [System.IO.File]::Create($d)
        $fs.SetLength(16MB)
        $fs.Close()
    }
}

Write-Output "=== Building kernel WITH research_authority_hw_demo ==="
Push-Location kernel_rs
$exitCode = Invoke-CargoQuiet @("build", "--release", "--target", "x86_64-unknown-none", "--features", "research_authority_hw_demo")
Pop-Location
if ($exitCode -ne 0) {
    Write-Error "Research-feature kernel build failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-drive", "if=none,id=ahcidisk,file=$AhciDiskImage,format=raw",
    "-device", "ide-hd,drive=ahcidisk,bus=ide.1",
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

# Always rebuild and redeploy the default (no-feature) kernel before
# checking results, so a failure here never leaves the tree stuck on
# the research build.
Write-Output "=== Rebuilding default kernel (feature off) ==="
Push-Location kernel_rs
Invoke-CargoQuiet @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Pop-Location
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Pattern = "AUTHORITY_HW_SELFCHECK_PASS"; Name = "section 1 (device): synthetic-slot software/hardware cross-check" },
    @{ Pattern = "AUTHORITY_HW_LIVE_FAULT_CONTROL first_command_completed=true"; Name = "section 1 (device): live AHCI control command completed (grant works)" },
    @{ Pattern = "AUTHORITY_HW_LIVE_FAULT_RESULT second_command_completed=false"; Name = "section 1 (device): post-revocation command did NOT complete" },
    @{ Pattern = "AUTHORITY_HW_LIVE_FAULT_PASS"; Name = "section 1 (device): real IOMMU fault captured after revocation" },
    @{ Pattern = "AUTHORITY_HW_CPU_PASS"; Name = "section 1 (CPU): real page-table state tracked the graph exactly" },
    @{ Pattern = "AUTHORITY_HW_CERT_PASS"; Name = "section 2: hardware-bound certificate detected real byte-level tampering" },
    @{ Pattern = "AUTHORITY_HW_ENVELOPE_PASS"; Name = "section 3: envelope discovered from real captured IOMMU faults, frozen, and enforced" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Authority-graph hardware revocation test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Authority-graph hardware wiring verified, sections 1-3: real IOMMU context-table state AND real CPU page-table state both track the pure authority_graph exactly (grant AND revoke); a real, live AHCI device's DMA attempt was blocked by real IOMMU hardware immediately after revocation; a certificate bound to real hardware bytes detected real byte-level tampering; and a least-privilege envelope discovered from REAL captured IOMMU fault addresses was frozen into real grants that correctly allow what was observed and deny what wasn't."
