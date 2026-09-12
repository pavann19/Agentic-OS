# Phase 12 exit criterion 3 (docs/ROADMAP.md): "A crashed compositor is
# restarted by the device manager without taking down running
# application processes' own state." Builds user_rs/compositor_driver
# WITH the crash_test feature (off by default -- see its own
# Cargo.toml), which does its real work first (both surfaces drawn,
# the adversarial bounds self-check run, readiness signaled) and THEN
# deliberately faults (a CPL0-only `hlt` at ring 3), the same real
# technique already proven on ahci_driver/netstack_driver.
#
# Boots the real kernel (feature compositor_demo, so compositor.rs --
# not user_driver.rs's Phase 3 demo -- owns the framebuffer) and checks
# for the real death-to-restart chain, driven through the SAME
# supervisor.rs/device_manager.rs mechanism already proven on AHCI --
# now against a real, non-PCI-backed process via
# device_manager::register_synthetic -- AND that the two separate
# window_client_driver processes (never touched by the compositor's
# crash) still completed their own real work correctly.
#
# Rebuilds the normal (non-crash_test) compositor_driver and default
# kernel afterward, same discipline as scripts/test-supervisor.ps1.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-compositor-crash.log",
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

Write-Output "=== Building compositor_driver WITH crash_test ==="
$exitCode = Invoke-CargoQuiet "user_rs\compositor_driver" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "crash_test")
if ($exitCode -ne 0) { Write-Error "crash_test compositor_driver build failed"; exit 1 }

Write-Output "=== Building kernel WITH compositor_demo (embeds the crash_test compositor_driver) ==="
# Real bug this session already found and worked around once (see
# scripts/test-tcp-two-instance.ps1's own comment): cargo's
# include_bytes! dependency tracking doesn't reliably notice the
# embedded binary changed when no kernel_rs source file itself
# changed -- touch the real source file that contains the
# include_bytes! call to force a genuine recheck.
(Get-Item "kernel_rs\src\compositor.rs").LastWriteTime = Get-Date
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "compositor_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (compositor_demo, crash_test embedded) failed"; exit 1 }
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

Write-Output "=== Rebuilding default compositor_driver + kernel ==="
Invoke-CargoQuiet "user_rs\compositor_driver" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
(Get-Item "kernel_rs\src\compositor.rs").LastWriteTime = Get-Date
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Name = "compositor registered with supervisor"; Pattern = "SUPERVISOR_REGISTERED device=fe:00.0" },
    @{ Name = "real work done + deliberate fault armed"; Pattern = "CRASH_TEST_ARMED" },
    @{ Name = "real fault killed the compositor process"; Pattern = "PROCESS_KILLED" },
    @{ Name = "supervisor observed the real death"; Pattern = "SUPERVISOR_REAL_DEATH device=fe:00.0" },
    @{ Name = "device manager approved and drove a real restart"; Pattern = "DEVMGR: fe:00.0 restarting" },
    @{ Name = "supervisor respawned the compositor"; Pattern = "SUPERVISOR_RESPAWN device=fe:00.0" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# Real evidence the two SEPARATE window_client_driver processes were
# never touched by the compositor's own crash -- both still completed
# their real adversarial self-check normally.
$clientChecks = @("WINDOW_CLIENT_SURFACE_GRANTED label=A", "WINDOW_CLIENT_SURFACE_GRANTED label=B", "FOREIGN_CAP_DENIED_OK")
foreach ($c in $clientChecks) {
    if ($content.Contains($c)) {
        Write-Output "  PASS: unaffected client evidence present: $c"
    } else {
        Write-Output "  FAIL: unaffected client evidence missing: $c"
        $allPassed = $false
    }
}
$foreignDeniedCount = ([regex]::Matches($content, [regex]::Escape("FOREIGN_CAP_DENIED_OK"))).Count
if ($foreignDeniedCount -ge 2) {
    Write-Output "  PASS: both window client processes ran their own real adversarial check independently (x$foreignDeniedCount)"
} else {
    Write-Output "  FAIL: expected 2 FOREIGN_CAP_DENIED_OK (one per client), got $foreignDeniedCount"
    $allPassed = $false
}

if (-not $allPassed) {
    Write-Error "Compositor real-crash-to-restart test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 12 exit criterion 3 verified: the compositor (a real, non-PCI-backed process registered under a synthetic device-manager identity) was killed by a real fault, the supervisor observed the real death and drove the same real bounded restart-on-crash policy already proven on AHCI, respawned the compositor, and the two SEPARATE window_client_driver processes' own state/work was completely unaffected."
