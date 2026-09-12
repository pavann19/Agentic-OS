# Phase 10 exit criterion 2 (docs/ROADMAP.md): "Two independent
# Agentic OS instances (two QEMU guests, then two Tier 2/3 machines)
# exchange TCP traffic directly." Real, disclosed scope for this
# increment: two QEMU guests (no physical hardware available yet --
# the roadmap's own next step, once Tier 2/3 hardware exists).
#
# QEMU's usermode networking (`-netdev user`, used by every other
# network test in this project) gives each guest its own private,
# isolated 10.0.2.0/24 -- by design, it never lets two guests reach
# each other. This script instead connects two SEPARATE QEMU
# processes' e1000 NICs via a real point-to-point Ethernet link
# (`-netdev socket`, a real TCP-tunneled L2 connection between the
# two host processes) -- both instances then have real, static,
# directly-routable IPs on the same subnet, no gateway/NAT involved,
# genuinely real TCP traffic flowing between two independent kernel
# instances.
#
# Builds netstack_driver TWICE (once with tcp_server_demo, once with
# tcp_client_demo -- mutually exclusive real roles, see its own
# Cargo.toml), each embedded into its own kernel build and its own
# FAT boot directory, then boots both simultaneously.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$BaseFatDir = "boot_rs\qemu_fatdir",
    [string]$ServerFatDir = "boot_rs\qemu_fatdir_tcp_server",
    [string]$ClientFatDir = "boot_rs\qemu_fatdir_tcp_client",
    [string]$ServerLog = "_evidence\latest\serial-tcp-server.log",
    [string]$ClientLog = "_evidence\latest\serial-tcp-client.log",
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

Write-Output "=== Building netstack_driver (server role) ==="
$exitCode = Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "tcp_server_demo")
if ($exitCode -ne 0) { Write-Error "netstack_driver (server) build failed"; exit 1 }

Write-Output "=== Building kernel (server role) ==="
# Real bug found and fixed: cargo's own dependency tracking for
# `include_bytes!` doesn't reliably notice the embedded
# netstack_driver binary changed between builds when nothing in
# kernel_rs's OWN source changed -- it served a stale cached kernel
# binary (still embedding the PREVIOUS role's netstack_driver) the
# second time around. Touching the real source file that contains the
# `include_bytes!` call forces cargo to genuinely re-check it.
(Get-Item "kernel_rs\src\netstack.rs").LastWriteTime = Get-Date
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "network_stack")
if ($exitCode -ne 0) { Write-Error "Kernel build (server) failed"; exit 1 }

if (Test-Path $ServerFatDir) { Remove-Item -Recurse -Force $ServerFatDir }
Copy-Item -Recurse $BaseFatDir $ServerFatDir
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$ServerFatDir\kernel.elf" -Force

Write-Output "=== Building netstack_driver (client role) ==="
$exitCode = Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "tcp_client_demo")
if ($exitCode -ne 0) { Write-Error "netstack_driver (client) build failed"; exit 1 }

Write-Output "=== Building kernel (client role) ==="
(Get-Item "kernel_rs\src\netstack.rs").LastWriteTime = Get-Date
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "network_stack")
if ($exitCode -ne 0) { Write-Error "Kernel build (client) failed"; exit 1 }

if (Test-Path $ClientFatDir) { Remove-Item -Recurse -Force $ClientFatDir }
Copy-Item -Recurse $BaseFatDir $ClientFatDir
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$ClientFatDir\kernel.elf" -Force

if (Test-Path $ServerLog) { Clear-Content $ServerLog }
if (Test-Path $ClientLog) { Clear-Content $ClientLog }

$serverArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$ServerFatDir,format=raw",
    "-netdev", "socket,id=net0,listen=:17171",
    "-device", "e1000,netdev=net0",
    "-serial", "file:$ServerLog",
    "-display", "none",
    "-no-reboot"
)
$clientArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$ClientFatDir,format=raw",
    "-netdev", "socket,id=net0,connect=127.0.0.1:17171",
    "-device", "e1000,netdev=net0",
    "-serial", "file:$ClientLog",
    "-display", "none",
    "-no-reboot"
)

Write-Output "=== Booting server instance ==="
$serverProc = Start-Process -FilePath $QemuExe -ArgumentList $serverArgs -PassThru -NoNewWindow
Start-Sleep -Seconds 3
Write-Output "=== Booting client instance ==="
$clientProc = Start-Process -FilePath $QemuExe -ArgumentList $clientArgs -PassThru -NoNewWindow

Start-Sleep -Seconds $BootWaitSeconds
foreach ($p in @($serverProc, $clientProc)) {
    if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
}

Write-Output "=== Rebuilding default netstack_driver + kernel ==="
Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
(Get-Item "kernel_rs\src\netstack.rs").LastWriteTime = Get-Date
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$BaseFatDir\kernel.elf" -Force

if (-not (Test-Path $ServerLog)) { Write-Error "No server serial output at $ServerLog"; exit 1 }
if (-not (Test-Path $ClientLog)) { Write-Error "No client serial output at $ClientLog"; exit 1 }
$serverContent = Get-Content $ServerLog -Raw
$clientContent = Get-Content $ClientLog -Raw

$allPassed = $true
$serverChecks = @("NETSTACK_TCP_SERVER_LISTENING", "NETSTACK_TCP_SERVER_ACCEPTED", "NETSTACK_TCP_SERVER_RECEIVED", "NETSTACK_TCP_SERVER_REPLIED")
foreach ($c in $serverChecks) {
    if ($serverContent.Contains($c)) {
        Write-Output "  PASS (server): $c"
    } else {
        Write-Output "  FAIL (server): pattern not found: $c"
        $allPassed = $false
    }
}
$clientChecks = @("NETSTACK_TCP_CLIENT_CONNECTING", "NETSTACK_TCP_CLIENT_CONNECTED", "NETSTACK_TCP_CLIENT_RECEIVED")
foreach ($c in $clientChecks) {
    if ($clientContent.Contains($c)) {
        Write-Output "  PASS (client): $c"
    } else {
        Write-Output "  FAIL (client): pattern not found: $c"
        $allPassed = $false
    }
}
if ($clientContent.Contains("HELLO_FROM_AGENTIC_OS_SERVER")) {
    Write-Output "  PASS: client received the real, distinct payload the SEPARATE server instance sent"
} else {
    Write-Output "  FAIL: client never received the server's real reply payload"
    $allPassed = $false
}
if ($serverContent.Contains("HELLO_FROM_AGENTIC_OS_CLIENT")) {
    Write-Output "  PASS: server received the real, distinct payload the SEPARATE client instance sent"
} else {
    Write-Output "  FAIL: server never received the client's real payload"
    $allPassed = $false
}

if (-not $allPassed) {
    Write-Error "Two-instance TCP exchange FAILED -- see $ServerLog and $ClientLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 10 exit criterion 2 verified: two SEPARATE, real Agentic OS kernel instances (two QEMU guests, connected via a real point-to-point Ethernet link, no shared usermode-networking subnet) completed a real TCP 3-way handshake and exchanged real, distinct application payloads in both directions."
