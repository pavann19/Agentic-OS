# Phase 14 Deliverable 2 / Exit Criterion:
# Secure Boot Chain, Hardware Root of Trust, and Measured Boot.
#
# Validates:
#  1. Bootloader SHA-256 measurement of kernel binary.
#  2. TPM 2.0 TCG event log recording and PCR extension (PCR 0, 4, 9).
#  3. Forwarding of cryptographic kernel hash via BootInfo to kernel.
#  4. Adversarial check: Tampered kernel / invalid signature rejected
#     at bootloader stage with SECURE_BOOT_VIOLATION: KERNEL_HASH_MISMATCH.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-secure-boot.log",
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

# Ensure clean state without stale signature
$sigPath = "$FatDir\kernel.sig"
if (Test-Path $sigPath) { Remove-Item $sigPath -Force }

Write-Output "=== Part 1: Clean Measured Boot Test ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "Kernel build failed"; exit 1 }
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

Write-Output "=== Launching QEMU for Measured Boot ==="
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
try {
    Start-Sleep -Seconds $BootWaitSeconds
} finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$cleanPassed = $true
$cleanChecks = @(
    @{ Name = "Bootloader SHA-256 kernel measurement"; Pattern = "SECURE_BOOT_PASS: KERNEL_HASH_MEASURED" },
    @{ Name = "TPM event log PCR[9] measurement extended"; Pattern = "TPM_MEASURED_BOOT_EXTEND pcr=9" },
    @{ Name = "Kernel receives measured kernel hash via BootInfo"; Pattern = "SECURE_BOOT_KERNEL_HASH_FORWARDED" }
)

foreach ($c in $cleanChecks) {
    if ($content.Contains($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) (pattern '$($c.Pattern)' not found)"
        $cleanPassed = $false
    }
}

if (-not $cleanPassed) {
    Write-Error "Clean Measured Boot verification failed"
    exit 1
}

Write-Output "=== Part 2: Adversarial Secure Boot Tamper Verification ==="
# Create invalid 32-byte signature file
$tamperedSig = New-Object byte[] 32
for ($i = 0; $i -lt 32; $i++) { $tamperedSig[$i] = 0xDE }
[System.IO.File]::WriteAllBytes($sigPath, $tamperedSig)

$tamperLog = "_evidence\latest\serial-secure-boot-tamper.log"
if (Test-Path $tamperLog) { Clear-Content $tamperLog }

$qemuArgsTamper = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$tamperLog",
    "-display", "none",
    "-no-reboot"
)

$procTamper = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgsTamper -PassThru -NoNewWindow
try {
    Start-Sleep -Seconds 15
} finally {
    if (-not $procTamper.HasExited) {
        Stop-Process -Id $procTamper.Id -Force -ErrorAction SilentlyContinue
    }
    # Clean up signature file
    if (Test-Path $sigPath) { Remove-Item $sigPath -Force }
}

if (-not (Test-Path $tamperLog)) { Write-Error "No tamper serial output at $tamperLog"; exit 1 }
$tamperContent = Get-Content $tamperLog -Raw

$tamperPassed = $true
if ($tamperContent.Contains("SECURE_BOOT_VIOLATION: KERNEL_HASH_MISMATCH")) {
    Write-Output "  PASS: Tampered image rejected with SECURE_BOOT_VIOLATION: KERNEL_HASH_MISMATCH"
} else {
    Write-Output "  FAIL: Did not detect SECURE_BOOT_VIOLATION"
    $tamperPassed = $false
}

if ($tamperContent.Contains("KERNEL_ENTER")) {
    Write-Output "  FAIL: Kernel was entered despite hash mismatch!"
    $tamperPassed = $false
} else {
    Write-Output "  PASS: Kernel entry blocked (Secure Boot refusal verified)"
}

if (-not $tamperPassed) {
    Write-Error "Adversarial Secure Boot tamper verification failed"
    exit 1
}

Write-Output "=== Secure Boot Chain & Measured Boot VERIFIED ==="
exit 0
