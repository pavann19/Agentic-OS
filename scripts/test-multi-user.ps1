# Phase 14 Deliverable 1 / Exit Criterion:
# Multi-User Capability Session Model (ADR-003: zero ambient authority).
#
# Validates:
#  1. Isolated user session initialization (Alice & Bob).
#  2. Adversarial cross-user isolation: Bob cannot name or discover Alice's private
#     object (NoSuchCapability, indistinguishable from nonexistence).
#  3. Explicit attenuated delegation: Alice delegates READ-only capability to Bob.
#  4. Adversarial rights escalation: Bob attempts WRITE on READ-only delegated capability (denied).
#  5. Immediate cross-user revocation: Alice revokes shared object, Bob immediately loses access (Revoked).

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-multi-user.log",
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

Write-Output "=== Building kernel WITH phase14_multi_user ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase14_multi_user")
if ($exitCode -ne 0) { Write-Error "Kernel build (phase14_multi_user) failed"; exit 1 }
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

Write-Output "=== Launching QEMU for Multi-User Session Demo ==="
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
    @{ Name = "Multi-user demo started"; Pattern = "MULTI_USER_DEMO_START" },
    @{ Name = "User sessions initialized (Alice & Bob)"; Pattern = "MULTI_USER_SESSION_INIT alice_uid=1000 bob_uid=1001" },
    @{ Name = "Alice private object created"; Pattern = "MULTI_USER_ALICE_PRIVATE_OBJECT" },
    @{ Name = "Adversarial cross-user denial (Bob cannot access Alice private cap)"; Pattern = "MULTI_USER_CROSS_ACCESS_DENIED_OK" },
    @{ Name = "Explicit attenuated delegation (Alice -> Bob READ-only)"; Pattern = "MULTI_USER_DELEGATION_OK" },
    @{ Name = "Bob resolves delegated shared capability successfully"; Pattern = "MULTI_USER_BOB_RESOLVE_SHARED_OK" },
    @{ Name = "Adversarial write denied on delegated read-only cap"; Pattern = "MULTI_USER_WRITE_DENIED_OK" },
    @{ Name = "Alice revokes shared object, Bob immediately revoked"; Pattern = "MULTI_USER_REVOCATION_OK" },
    @{ Name = "Multi-user demo success"; Pattern = "MULTI_USER_DEMO_SUCCESS" }
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
    Write-Error "Multi-user verification failed"
    exit 1
}

Write-Output "=== Multi-User Capability Session Model VERIFIED ==="
exit 0
