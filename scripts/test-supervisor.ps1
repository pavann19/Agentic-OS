# Phase 9.5a (docs/ROADMAP.md Sec5): real crash-to-restart supervision.
# Builds user_rs/ahci_driver WITH the crash_test feature (off by
# default -- see that crate's own Cargo.toml comment), which does its
# REAL work first (real IDENTIFY DEVICE, real decoded model string)
# and THEN deliberately faults (a CPL0-only `hlt` at ring 3, the same
# technique fault_isolation_demo.rs already proved raises a real #GP).
# Boots the real kernel against it and checks for the REAL death-to-
# restart chain -- SUPERVISOR_REAL_DEATH -> DEVMGR crash/restart ->
# SUPERVISOR_RESPAWN -> a SECOND real AHCI_SELF_CHECK_PASS from the
# respawned process -- never the removed simulated path
# (DEVMGR_SIMULATED_CRASH must NOT appear; that call site is gone).
# Rebuilds the normal (non-crash_test) ahci_driver and default kernel
# afterward, same discipline as scripts/test-authority-revoke.ps1, so
# the tree is left in its normal state.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-supervisor.log",
    [int]$BootWaitSeconds = 20
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

Write-Output "=== Building ahci_driver WITH crash_test ==="
$exitCode = Invoke-CargoQuiet "user_rs\ahci_driver" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "crash_test")
if ($exitCode -ne 0) {
    Write-Error "crash_test ahci_driver build failed"
    exit 1
}

Write-Output "=== Building kernel embedding the crash_test ahci_driver ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "Kernel build (crash_test embedded) failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (Test-Path $SerialLog) { Clear-Content $SerialLog }
$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

# Always rebuild the normal (non-crash_test) driver and default kernel
# before checking results, so a failure here never leaves the tree
# stuck on the deliberately-crashing build.
Write-Output "=== Rebuilding normal ahci_driver + default kernel ==="
Invoke-CargoQuiet "user_rs\ahci_driver" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Pattern = "AHCI_SELF_CHECK_PASS"; Name = "first (crashing) driver instance did its real work first"; MinCount = 1 },
    @{ Pattern = "CRASH_TEST_ARMED"; Name = "deliberate real fault armed" },
    @{ Pattern = "PROCESS_KILLED: fault occurred in ring 3"; Name = "real ring-3 fault caught by idt.rs (not simulated)" },
    @{ Pattern = "SUPERVISOR_REAL_DEATH device=00:1f.2"; Name = "supervisor observed the REAL death and traced it to the real device" },
    @{ Pattern = "DEVMGR: 00:1f.2 crashed"; Name = "real device-manager crash transition, driven by the real death" },
    @{ Pattern = "DEVMGR: 00:1f.2 restarting (attempt 1/3)"; Name = "real bounded restart decision (kernel_common::supervision::decide_restart)" },
    @{ Pattern = "SUPERVISOR_RESPAWN device=00:1f.2"; Name = "supervisor actually respawned the real driver process" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# Real, necessary negative check: the OLD simulated-crash call site is
# gone -- if this string ever appears again, real and simulated
# evidence would be ambiguous, exactly what this phase exists to avoid.
if ($content -match [regex]::Escape("DEVMGR_SIMULATED_CRASH")) {
    Write-Output "  FAIL: DEVMGR_SIMULATED_CRASH appeared -- the simulated call site should be removed"
    $allPassed = $false
} else {
    Write-Output "  PASS: no simulated-crash call site present (removed, per device_manager.rs's own updated doc)"
}

# Real evidence the RESTARTED process is a genuinely NEW, working
# instance, not a hung/half-dead one: a SECOND AHCI_SELF_CHECK_PASS
# after the respawn (the respawned instance also crashes again, since
# it's the same crash_test build -- this run only checks for the first
# real recovery, not the eventual quarantine after 3 attempts).
$passCount = ([regex]::Matches($content, [regex]::Escape("AHCI_SELF_CHECK_PASS"))).Count
if ($passCount -ge 2) {
    Write-Output "  PASS: the respawned driver process did real work again (AHCI_SELF_CHECK_PASS x$passCount) -- a genuine restart, not a hang"
} else {
    Write-Output "  FAIL: only $passCount AHCI_SELF_CHECK_PASS -- the respawned process never did real work"
    $allPassed = $false
}

if (-not $allPassed) {
    Write-Error "Supervisor real-crash-to-restart test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 9.5a supervisor verified: a real user-space driver process was killed by a real fault (not simulated), the supervisor observed the real death and traced it to the real PCI device, drove the real bounded restart-on-crash policy, and respawned the real driver process, which did real work again."
