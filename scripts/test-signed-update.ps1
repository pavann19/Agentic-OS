# Phase 14 Deliverable 3 / Exit Criterion:
# Cryptographically Signed Updates with Dual-Bank A/B Rollback.
#
# Validates:
#  1. Cryptographically signed package (AGYPKG2) format verification.
#  2. SHA-256 payload integrity validation.
#  3. HMAC-SHA256 signature authentication using hardware root key.
#  4. Monotonic anti-rollback version enforcement (rejects downgrades).
#  5. Adversarial check 1: Tampered payload rejected on SHA-256 mismatch.
#  6. Adversarial check 2: Bad signature rejected on HMAC mismatch.
#  7. Adversarial check 3: Downgrade attack rejected.
#  8. Dual-bank A/B state machine rollback execution.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-signed-update.log",
    [int]$BootWaitSeconds = 25
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

Write-Output "=== Building kernel WITH phase14_signed_update ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase14_signed_update")
if ($exitCode -ne 0) { Write-Error "Kernel build (phase14_signed_update) failed"; exit 1 }
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

Write-Output "=== Launching QEMU for Signed Update & Dual-Bank Rollback Demo ==="
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
    @{ Name = "Signed update demo started"; Pattern = "SIGNED_UPDATE_DEMO_START" },
    @{ Name = "Valid v2 signed package accepted and committed to Bank B"; Pattern = "SIGNED_UPDATE_V2_ACCEPTED_OK bank=BankB version=2" },
    @{ Name = "Tampered payload rejected on SHA-256 mismatch"; Pattern = "SIGNED_UPDATE_TAMPERED_REJECTED_OK (Sha256Mismatch)" },
    @{ Name = "Bad signature rejected on HMAC mismatch"; Pattern = "SIGNED_UPDATE_BAD_SIGNATURE_REJECTED_OK (HmacMismatch)" },
    @{ Name = "Version downgrade rejected by anti-rollback counter"; Pattern = "SIGNED_UPDATE_DOWNGRADE_REJECTED_OK (old=2 new=1)" },
    @{ Name = "Dual-bank rollback succeeds (restores Bank A v1)"; Pattern = "SIGNED_UPDATE_ROLLBACK_OK active=BankA restored_version=1" },
    @{ Name = "Signed update demo success"; Pattern = "SIGNED_UPDATE_DEMO_SUCCESS" }
)

foreach ($c in $checks) {
    $matched = $content.Contains($c.Pattern)
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) (pattern '$($c.Pattern)' not found)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Signed update verification failed"
    exit 1
}

Write-Output "=== Cryptographically Signed Updates & Dual-Bank Rollback VERIFIED ==="
exit 0
