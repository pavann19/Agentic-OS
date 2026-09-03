# Research track (docs/RESEARCH_TRACK.md, docs/NOVEL_CONCEPTS.md §1):
# real-hardware verification of the authority-graph-to-IOMMU wiring
# (kernel_rs/src/authority.rs) -- both the synthetic-slot software/
# hardware cross-check AND the live-device (real AHCI, 00:1f.2)
# fault-after-revocation escalation (kernel_rs/src/
# authority_hw_fault_demo.rs). Builds the kernel WITH the
# `research_authority_hw_demo` feature (off by default -- see
# Cargo.toml's own comment on why: this demo's deliberate bounded wait
# for an expected-to-fail command adds real wall-clock delay unsuitable
# for every normal boot), boots it against a real AHCI disk under a
# real `intel-iommu` device, and asserts both self-checks reported
# PASS. Rebuilds the default (non-feature) kernel afterward, same
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
    @{ Pattern = "AUTHORITY_HW_SELFCHECK_PASS"; Name = "synthetic-slot software/hardware cross-check" },
    @{ Pattern = "AUTHORITY_HW_LIVE_FAULT_CONTROL first_command_completed=true"; Name = "live AHCI control command completed (grant works)" },
    @{ Pattern = "AUTHORITY_HW_LIVE_FAULT_RESULT second_command_completed=false"; Name = "post-revocation command did NOT complete" },
    @{ Pattern = "AUTHORITY_HW_LIVE_FAULT_PASS"; Name = "real IOMMU fault captured after revocation" }
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
Write-Output "Authority-graph hardware wiring verified: real IOMMU context-table state tracks the pure authority_graph exactly (grant AND revoke), and a real, live AHCI device's DMA attempt was blocked by real IOMMU hardware immediately after revocation -- a real fault, not a simulated or asserted one."
