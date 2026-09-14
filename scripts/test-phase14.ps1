# Comprehensive Phase 14 Test Suite:
# "Multi-User, Update Integrity, And Hardware Root Of Trust"
#
# Validates:
#  1. Host unit tests (pure no_std SHA-256, HMAC-SHA256, ChaCha20, TPM event log).
#  2. Secure Boot chain & Measured Boot (bootloader kernel measurement & tamper rejection).
#  3. Multi-User capability session model (ADR-003: zero ambient authority, isolated user domains).
#  4. Cryptographically signed updates (AGYPKG2) with dual-bank A/B rollback.
#  5. Full-disk / object-store encryption with ChaCha20 and sector binding.
#  6. Local-first crash/panic telemetry (structured serialization and SHA-256 validation).

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-phase14-all.log",
    [int]$BootWaitSeconds = 30
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

Write-Output "============================================================"
Write-Output "               PHASE 14 COMPREHENSIVE SUITE                 "
Write-Output "============================================================"

# Step 1: Host unit tests
Write-Output "`n[1/6] Running host crypto and TPM unit tests..."
Push-Location "host_tests"
try {
    & cargo test -- --quiet
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Host tests failed!"
        exit 1
    }
} finally {
    Pop-Location
}
Write-Output "  PASS: Host unit tests"

# Step 2: Secure Boot & Measured Boot test
Write-Output "`n[2/6] Running Secure Boot & Measured Boot test..."
& powershell -ExecutionPolicy Bypass -File "scripts\test-secure-boot.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "test-secure-boot.ps1 failed!"
    exit 1
}

# Step 3: Multi-User capability session test
Write-Output "`n[3/6] Running Multi-User Capability Session test..."
& powershell -ExecutionPolicy Bypass -File "scripts\test-multi-user.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "test-multi-user.ps1 failed!"
    exit 1
}

# Step 4: Signed Updates with Dual-Bank Rollback test
Write-Output "`n[4/6] Running Signed Updates & Dual-Bank Rollback test..."
& powershell -ExecutionPolicy Bypass -File "scripts\test-signed-update.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "test-signed-update.ps1 failed!"
    exit 1
}

# Step 5: Full-Disk Encryption test
Write-Output "`n[5/6] Running Full-Disk / Object-Store Encryption test..."
& powershell -ExecutionPolicy Bypass -File "scripts\test-crypto-store.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "test-crypto-store.ps1 failed!"
    exit 1
}

# Step 6: Combined Phase 14 All-Features test (including Telemetry)
Write-Output "`n[6/6] Running Combined phase14_all integration test..."
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase14_all")
if ($exitCode -ne 0) { Write-Error "Kernel build (phase14_all) failed"; exit 1 }
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

# Restore default kernel build
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$allChecks = @(
    @{ Name = "Secure boot kernel hash forwarded"; Pattern = "SECURE_BOOT_KERNEL_HASH_FORWARDED" },
    @{ Name = "Multi-user demo success"; Pattern = "MULTI_USER_DEMO_SUCCESS" },
    @{ Name = "Signed update demo success"; Pattern = "SIGNED_UPDATE_DEMO_SUCCESS" },
    @{ Name = "Crypto store demo success"; Pattern = "CRYPTO_STORE_DEMO_SUCCESS" },
    @{ Name = "Telemetry record captured"; Pattern = "TELEMETRY_RECORD_CAPTURED" },
    @{ Name = "Telemetry record verified (SHA-256)"; Pattern = "TELEMETRY_RECORD_VERIFIED_OK" },
    @{ Name = "Telemetry local-first stored"; Pattern = "TELEMETRY_LOCAL_FIRST_STORED_OK" },
    @{ Name = "Telemetry demo success"; Pattern = "TELEMETRY_DEMO_SUCCESS" }
)

$allPassed = $true
foreach ($c in $allChecks) {
    if ($content.Contains($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) (pattern '$($c.Pattern)' not found)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Combined phase14_all verification failed"
    exit 1
}

Write-Output "`n============================================================"
Write-Output "        PHASE 14 VERIFICATION 100% COMPLETE & PASSING      "
Write-Output "============================================================"
exit 0
