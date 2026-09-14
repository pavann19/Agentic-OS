# Phase 14 Deliverable 4 / Exit Criterion:
# Full-Disk / Object-Store Encryption using ChaCha20 Stream Cipher.
#
# Validates:
#  1. ChaCha20 256-bit stream cipher sector encryption.
#  2. Sector LBA cryptographic nonce binding.
#  3. Raw disk ciphertext verification: zero plaintext leakage on media.
#  4. Byte-identical round-trip decryption with master key.
#  5. Adversarial check 1: Wrong key fails decryption (garbage output).
#  6. Adversarial check 2: Cross-sector relocation attack rejected by sector binding.
#  7. Capability-gated access: DECRYPT right enforced by capability table.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-crypto-store.log",
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

Write-Output "=== Building kernel WITH phase14_crypto_store ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase14_crypto_store")
if ($exitCode -ne 0) { Write-Error "Kernel build (phase14_crypto_store) failed"; exit 1 }
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

Write-Output "=== Launching QEMU for Full-Disk Encryption Demo ==="
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
    @{ Name = "Crypto store demo started"; Pattern = "CRYPTO_STORE_DEMO_START sector=42" },
    @{ Name = "Sector encrypted with ChaCha20"; Pattern = "CRYPTO_STORE_ENCRYPT_OK sector=42" },
    @{ Name = "Raw disk ciphertext verified (zero plaintext leakage)"; Pattern = "CRYPTO_STORE_CIPHERTEXT_VERIFIED" },
    @{ Name = "Decryption round-trip byte-identical match"; Pattern = "CRYPTO_STORE_DECRYPT_ROUNDTRIP_OK (byte-identical match)" },
    @{ Name = "Wrong key decryption rejected (produces garbage)"; Pattern = "CRYPTO_STORE_WRONG_KEY_REJECTED_OK" },
    @{ Name = "Cross-sector relocation attack detected by LBA binding"; Pattern = "CRYPTO_STORE_CROSS_SECTOR_TAMPER_DETECTED_OK" },
    @{ Name = "Capability-gated access check passed"; Pattern = "CRYPTO_STORE_CAPABILITY_CHECK_OK" },
    @{ Name = "Crypto store demo success"; Pattern = "CRYPTO_STORE_DEMO_SUCCESS" }
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
    Write-Error "Crypto store verification failed"
    exit 1
}

Write-Output "=== Full-Disk / Object-Store Encryption VERIFIED ==="
exit 0
