# Phase 12 (docs/ROADMAP.md Sec5, deliverable 5 & exit criterion 2): Typed Agent UI Introspection & Direct Action.
# Boots the OS in QEMU and validates that an agent process holding Rights::INTROSPECT
# can enumerate windows as typed data (SYS_INTROSPECT_WINDOWS) and inject typed actions
# (SYS_AGENT_UI_ACTION) without pixel-scraping or OCR, and verifies that unauthorized
# agent processes are strictly denied and audited.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-agent-ui.log",
    [int]$BootWaitSeconds = 15
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

Write-Output "=== Building agent_demo (release) ==="
$exitCode = Invoke-CargoQuiet "user_rs\agent_demo" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "agent_demo build failed"
    exit 1
}

Write-Output "=== Building kernel (default) ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "Kernel build failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

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

Write-Output "=== Launching QEMU for Agent UI Introspection test ==="
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    "WINDOW_INTROSPECT_OK",
    "SYSCALL_INTROSPECT_WINDOWS_OK",
    "AGENT_UI_ACTION_PASS",
    "SYSCALL_AGENT_UI_ACTION_OK",
    "WINDOW_INTROSPECT_DENIED",
    "AGENT_UI_ACTION_DENIED_OK",
    "SYSCALL_INTROSPECT_WINDOWS_DENIED",
    "SYSCALL_AGENT_UI_ACTION_DENIED"
)

foreach ($c in $checks) {
    if ($content.Contains($c)) {
        Write-Output "  PASS: $c"
    } else {
        Write-Output "  FAIL: pattern not found: $c"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Agent UI Introspection test FAILED -- check log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "=== Typed Agent UI Introspection & Direct Action VERIFIED (Phase 12 Deliverable 5 & Exit Criterion 2 PASS) ==="
