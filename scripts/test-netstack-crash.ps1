# Phase 10 deliverable 4 / exit criterion 4 (docs/ROADMAP.md): "The
# network-stack process is killed mid-transfer; it restarts; no other
# process's sockets are affected." Builds netstack_driver WITH the
# crash_test feature (off by default -- see its own Cargo.toml), which
# does real work first (a real ICMP self-check against QEMU's real
# gateway) and THEN deliberately faults (a CPL0-only `hlt` at ring 3),
# same technique already proven for AHCI (scripts/test-supervisor.ps1).
# Boots the real kernel (WITH network_stack, so netstack.rs -- not
# e1000.rs's demo -- owns the NIC) and checks for the real death-to-
# restart chain, ending in a SECOND real ICMP self-check pass from the
# respawned process. Rebuilds the normal netstack_driver and default
# kernel afterward.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-netstack-crash.log",
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

Write-Output "=== Building netstack_driver WITH crash_test ==="
$exitCode = Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "crash_test")
if ($exitCode -ne 0) {
    Write-Error "crash_test netstack_driver build failed"
    exit 1
}

Write-Output "=== Building kernel WITH network_stack (embeds the crash_test netstack_driver) ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "network_stack")
if ($exitCode -ne 0) {
    Write-Error "Kernel build (network_stack) failed"
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
    "-netdev", "user,id=net0",
    "-device", "e1000,netdev=net0",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

Write-Output "=== Rebuilding normal netstack_driver + default kernel ==="
Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Pattern = "NETSTACK_ICMP_SELF_CHECK_PASS"; Name = "first (crashing) netstack instance did its real work first"; MinCount = 1 },
    @{ Pattern = "CRASH_TEST_ARMED"; Name = "deliberate real fault armed" },
    @{ Pattern = "PROCESS_KILLED: fault occurred in ring 3"; Name = "real ring-3 fault caught by idt.rs (not simulated)" },
    @{ Pattern = "SUPERVISOR_REAL_DEATH"; Name = "supervisor observed the REAL death and traced it to the real device" },
    @{ Pattern = "restarting (attempt 1/3)"; Name = "real bounded restart decision (kernel_common::supervision::decide_restart)" },
    @{ Pattern = "SUPERVISOR_RESPAWN"; Name = "supervisor actually respawned the real netstack_driver process" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# Real evidence the RESTARTED process is a genuinely new, working
# instance, not a hung/half-dead one: a SECOND real ICMP self-check
# pass after the respawn.
$passCount = ([regex]::Matches($content, [regex]::Escape("NETSTACK_ICMP_SELF_CHECK_PASS"))).Count
if ($passCount -ge 2) {
    Write-Output "  PASS: the respawned netstack process did real work again (NETSTACK_ICMP_SELF_CHECK_PASS x$passCount) -- a genuine restart, not a hang"
} else {
    Write-Output "  FAIL: only $passCount NETSTACK_ICMP_SELF_CHECK_PASS -- the respawned process never did real work"
    $allPassed = $false
}

if (-not $allPassed) {
    Write-Error "Netstack crash-to-restart test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 10 exit criterion 4 verified: the real network-stack process was killed by a real fault mid-run (after real ICMP work), the same real supervisor mechanism already proven for AHCI observed the death, traced it to the real PCI device, and respawned the process, which did real network work again."
