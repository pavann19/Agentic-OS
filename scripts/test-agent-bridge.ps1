# Live Agent Bridge Verification Script (scripts/test-agent-bridge.ps1)
# Verifies end-to-end communication between external host/LLM and running Agentic OS:
# 1. Boots QEMU with COM1 file logging and COM2 live TCP socket server.
# 2. Issues ListWindows request over COM2, asserting typed window records returned.
# 3. Issues InjectKey and FocusWindow requests over COM2, asserting successful action dispatch.
# 4. Verifies adversarial denial: gateway process without Rights::INTROSPECT is refused and audited.
# 5. Restores default kernel build.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-agent-bridge.log",
    [int]$BridgePort = 4444,
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

Write-Output "=== Building user_rs/agent_gateway (release) ==="
$exitCode = Invoke-CargoQuiet "user_rs\agent_gateway" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "agent_gateway build failed"
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
    "-chardev", "socket,id=agentchan,host=127.0.0.1,port=$BridgePort,server=on,wait=off",
    "-device", "isa-serial,chardev=agentchan",
    "-display", "none",
    "-no-reboot"
)

Write-Output "=== Launching QEMU with COM1 logging & COM2 Live Agent Bridge (port $BridgePort) ==="
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds

$allPassed = $true

try {
    Write-Output "--- Test 1: Sending ListWindows over COM2 bridge ---"
    $respList = & powershell -ExecutionPolicy Bypass -File scripts\agent-bridge-send.ps1 -Port $BridgePort -Command ListWindows
    Write-Output $respList
    if ($respList -match "OK: \d+ active window\(s\) enumerated" -and $respList -match "Agent Workspace") {
        Write-Output "  PASS: ListWindows returned active window data matching compositor"
    } else {
        Write-Output "  FAIL: ListWindows did not return expected window data"
        $allPassed = $false
    }

    Write-Output "--- Test 2: Sending InjectKey over COM2 bridge ---"
    $respKey = & powershell -ExecutionPolicy Bypass -File scripts\agent-bridge-send.ps1 -Port $BridgePort -Command InjectKey -Surface 1 -Arg0 0x1E
    Write-Output $respKey
    if ($respKey -match "OK: Action completed successfully") {
        Write-Output "  PASS: InjectKey executed successfully"
    } else {
        Write-Output "  FAIL: InjectKey action failed"
        $allPassed = $false
    }

    Write-Output "--- Test 3: Sending FocusWindow over COM2 bridge ---"
    $respFocus = & powershell -ExecutionPolicy Bypass -File scripts\agent-bridge-send.ps1 -Port $BridgePort -Command FocusWindow -Surface 1
    Write-Output $respFocus
    if ($respFocus -match "OK: Action completed successfully") {
        Write-Output "  PASS: FocusWindow executed successfully"
    } else {
        Write-Output "  FAIL: FocusWindow action failed"
        $allPassed = $false
    }
} finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$logContent = Get-Content $SerialLog -Raw

$com1Checks = @(
    "COM2_FOUND",
    "AGENT_GATEWAY_SPAWN",
    "AGENT_GATEWAY_PORTS_GRANTED",
    "AGENT_GATEWAY_ELF_ENTER",
    "REQUEST: ListWindows",
    "SYS_INTROSPECT_WINDOWS OK",
    "REQUEST: InjectKey",
    "AGENT_UI_ACTION_INJECT_KEY surface=1 scancode=0x1e"
)
foreach ($c in $com1Checks) {
    if ($logContent.Contains($c)) {
        Write-Output "  PASS: COM1 log check confirmed '$c'"
    } else {
        Write-Output "  FAIL: COM1 log pattern not found: '$c'"
        $allPassed = $false
    }
}

# -------------------------------------------------------------
# Test 4: Adversarial Case -- Gateway without Rights::INTROSPECT
# -------------------------------------------------------------
Write-Output ""
Write-Output "=== Test 4: Adversarial Test (agent_gateway without Rights::INTROSPECT) ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "agent_gateway_unauthorized")
if ($exitCode -ne 0) {
    Write-Error "Kernel build (agent_gateway_unauthorized) failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force
Clear-Content $SerialLog

$procAdv = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds

try {
    Write-Output "--- Sending ListWindows to unauthorized gateway ---"
    $respAdv = & powershell -ExecutionPolicy Bypass -File scripts\agent-bridge-send.ps1 -Port $BridgePort -Command ListWindows
    Write-Output $respAdv
    if ($respAdv -match "DENIED: In-guest kernel capability check rejected request") {
        Write-Output "  PASS: Unauthorized gateway request refused over COM2 wire with RespDenied"
    } else {
        Write-Output "  FAIL: Expected RespDenied from unauthorized gateway"
        $allPassed = $false
    }
} finally {
    if (-not $procAdv.HasExited) {
        Stop-Process -Id $procAdv.Id -Force -ErrorAction SilentlyContinue
    }
}

$advLog = Get-Content $SerialLog -Raw
$advChecks = @(
    "AGENT_GATEWAY_SPAWN (UNAUTHORIZED ADVERSARIAL)",
    "SYSCALL_INTROSPECT_WINDOWS_DENIED",
    "SYS_INTROSPECT_WINDOWS DENIED"
)
foreach ($c in $advChecks) {
    if ($advLog.Contains($c)) {
        Write-Output "  PASS: COM1 log check confirmed '$c'"
    } else {
        Write-Output "  FAIL: COM1 log pattern not found: '$c'"
        $allPassed = $false
    }
}

# Restore default kernel build
Write-Output "=== Restoring default (non-feature) kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not $allPassed) {
    Write-Error "Live Agent Bridge test FAILED -- check log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "================================================================="
Write-Output "=== Live Agent Bridge VERIFIED (100% PASS) ==="
Write-Output "  - Dedicated UART COM2 (0x2F8) transport verified"
Write-Output "  - Ring-3 agent_gateway typed request/response round-trip verified"
Write-Output "  - SYS_INTROSPECT_WINDOWS & SYS_AGENT_UI_ACTION dispatched live"
Write-Output "  - Adversarial denial and audit logging confirmed"
Write-Output "================================================================="
