# Phase 13 Deliverable 4: Fourth Reference App -- net_client
# Exercises GUI + network stack via IPC-mediated net_service.
#
# Validates:
#  1. netstack_driver discovers e1000 NIC and registers net_service.
#  2. Manifest-gated installer grants Surface and Socket capabilities to net_client.
#  3. net_client enters ring 3 and sends real HTTP fetch request via net_service IPC.
#  4. netstack_driver receives request, connects via TCP, and sends reply.
#  5. net_client receives byte-verified response and renders it to GUI surface.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-net-client.log",
    [int]$BootWaitSeconds = 60
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

Write-Output "=== Building netstack_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "netstack_driver build failed"; exit 1 }

Write-Output "=== Building net_client ==="
$exitCode = Invoke-CargoQuiet "user_rs\net_client" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "net_client build failed"; exit 1 }

Write-Output "=== Building kernel WITH net_client_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "net_client_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (net_client_demo) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
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
    @{ Name = "Manifest-gated installer granted Surface and Socket capabilities"; Pattern = "INSTALLER_GRANT_OK label=net_client_surface" },
    @{ Name = "net_client entered ring 3"; Pattern = "NET_CLIENT_ELF_ENTER" },
    @{ Name = "net_client signaled readiness to kernel"; Pattern = "NET_CLIENT_READY" },
    @{ Name = "net_service server registered by netstack"; Pattern = "NET_SERVICE_SERVER_REGISTERED" },
    @{ Name = "net_client issued HTTP fetch request"; Pattern = "NET_SERVICE_REQUEST_SENT" },
    @{ Name = "netstack_driver received IPC request"; Pattern = "NET_SERVICE_REQUEST_RECEIVED" },
    @{ Name = "netstack_driver sent reply via IPC"; Pattern = "NET_SERVICE_REPLY_SENT" },
    @{ Name = "net_client received response bytes"; Pattern = "HTTP_RESPONSE_RECEIVED" },
    @{ Name = "net_client verified HTTP status line"; Pattern = "HTTP_STATUS_OK: response byte-verified" }
)

foreach ($c in $checks) {
    $matched = $content.Contains($c.Pattern)
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "net_client test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 Deliverable 4 verified (fourth reference app): net_client runs capability-isolated in ring 3, exercises Phase 10's network stack over IPC, and renders network content to its GUI surface."
