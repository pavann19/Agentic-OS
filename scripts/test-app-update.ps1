# Phase 13 Deliverables 3 & 5 / Exit Criterion 4:
# Reproducible Package Format and Adversarial App Update Verification
#
# Validates:
#  1. Package header parsing & Adler-32 integrity validation (v1).
#  2. Clean update verification: version 1 -> version 2 monotonic increment
#     matching reproducible source hash.
#  3. Adversarial Check 1: Tampered package (bit flip in instruction) rejected with ChecksumMismatch.
#  4. Adversarial Check 2: Non-reproducible update (binary hash mismatch against declared source) rejected.
#  5. Adversarial Check 3: Version rollback / downgrade attack (v2 -> v1) rejected with UpdateVersionDowngrade.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-app-update.log",
    [int]$BootWaitSeconds = 40
)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
[Environment]::CurrentDirectory = $repoRoot

$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

function Invoke-CargoQuiet {
    param([string]$WorkDir, [string[]]$CargoArgs)
    Push-Location $WorkDir
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo @CargoArgs 2>&1 | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $saved
        Pop-Location
    }
}

Write-Output "=== Building kernel WITH app_update_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "app_update_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (app_update_demo) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Clear-Content $SerialLog }

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
try {
    Start-Sleep -Seconds $BootWaitSeconds
} finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

Write-Output "=== Rebuilding default kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Name = "App update demo started"; Pattern = "APP_UPDATE_DEMO_START" },
    @{ Name = "Package v1 parsed and verified"; Pattern = "APP_UPDATE_V1_VERIFY_PASS" },
    @{ Name = "Clean v2 update accepted with monotonic version and valid source hash"; Pattern = "APP_UPDATE_V2_ACCEPTED_PASS" },
    @{ Name = "Tampered payload rejected on checksum mismatch"; Pattern = "APP_UPDATE_TAMPERED_PAYLOAD_REJECTED_PASS" },
    @{ Name = "Non-reproducible update rejected against declared source hash"; Pattern = "APP_UPDATE_NON_REPRODUCIBLE_REJECTED_PASS" },
    @{ Name = "Version downgrade rejected"; Pattern = "APP_UPDATE_DOWNGRADE_REJECTED_PASS" },
    @{ Name = "App update test suite completed cleanly"; Pattern = "APP_UPDATE_DEMO_DONE" }
)

foreach ($c in $checks) {
    $matched = $content.Contains($c.Pattern)
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "App update test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 Deliverables 3 & 5 / Exit Criterion 4 verified: Package format parsed, reproducible builds verified against declared source, and tampering/downgrades rejected with real kernel audit records."
