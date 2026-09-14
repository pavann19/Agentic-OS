# Phase 11 Deliverable 3:
# ACPI Power Management, CPU C-States, and S3 Suspend-to-RAM State Machine.
#
# Validates:
#  1. ACPI FADT (Fixed ACPI Description Table, signature "FACP") discovery and parsing.
#  2. Power management register blocks: PM1a_CNT_BLK, PM1b_CNT_BLK, PM_TMR_BLK.
#  3. CPU C-state policy configuration: C1 halt (HLT) and C1E (MWAIT).
#  4. S3 Suspend-to-RAM state machine transitions: S0Active -> S3Preparing -> S3Suspended -> S3Restoring -> S0Active.
#  5. Architectural CPU register capture and restoration (GPRs, CR0/CR3/CR4, GDTR, IDTR, Segments, MSRs).
#  6. Pre-suspend memory fingerprinting and post-resume verification: zero bytes corrupted.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-power.log",
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

Write-Output "=== Building kernel WITH phase11_power ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase11_power")
if ($exitCode -ne 0) { Write-Error "Kernel build (phase11_power) failed"; exit 1 }
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

Write-Output "=== Launching QEMU for ACPI Power Management & S3 Suspend/Resume Demo ==="
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
    @{ Name = "Power management initialization started"; Pattern = "POWER_MANAGEMENT_INIT_START" },
    @{ Name = "FADT discovered / PM1 control parsed"; Pattern = "POWER_ACPI_FADT" },
    @{ Name = "CPU C-states configured"; Pattern = "POWER_CPU_CSTATES_CONFIGURED" },
    @{ Name = "Power transition to S3Preparing"; Pattern = "POWER_STATE_TRANSITION state=S3Preparing" },
    @{ Name = "S3 suspend initiated with state capture"; Pattern = "POWER_S3_SUSPEND_START" },
    @{ Name = "Memory fingerprint pre-suspend computed"; Pattern = "POWER_S3_PRE_SUSPEND_HASH" },
    @{ Name = "Power transition to S3Suspended"; Pattern = "POWER_STATE_TRANSITION state=S3Suspended" },
    @{ Name = "Power transition to S3Restoring"; Pattern = "POWER_STATE_TRANSITION state=S3Restoring" },
    @{ Name = "Architectural registers restored"; Pattern = "POWER_S3_RESUME_RESTORED_OK" },
    @{ Name = "Memory integrity verified (0 bytes corrupted)"; Pattern = "POWER_S3_MEMORY_INTEGRITY_VERIFIED" },
    @{ Name = "Power transition back to S0Active"; Pattern = "POWER_STATE_TRANSITION state=S0Active" },
    @{ Name = "Power management pass"; Pattern = "POWER_MANAGEMENT_PASS" }
)

foreach ($c in $checks) {
    $matched = $content.Contains($c.Pattern)
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- expected pattern: '$($c.Pattern)'"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "ACPI Power Management & S3 Suspend verification FAILED. See log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "ACPI Power Management, CPU C-states, and S3 Suspend/Resume verified end-to-end with register and memory integrity preservation."
