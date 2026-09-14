# Phase 10 (docs/ROADMAP.md Sec5): Multi-NIC Routing, Loopback & DNS Capability Verification
# Builds the kernel WITH the network_stack feature and boots it against QEMU's
# user-mode network. Verifies:
# 1. DNS capability-object wiring: authorized process succeeds, un-held capability is denied and audited.
# 2. Multi-NIC routing table with Longest Prefix Match (LPM) over IF_LOOPBACK, IF_ETH0, and IF_WLAN0.
# 3. In-memory loopback processing (127.0.0.1) without hitting the physical NIC.
# 4. Standard ICMP, DNS, TCP, and HTTP full-stack communication.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-multi-nic.log",
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
if ($exitCode -ne 0) {
    Write-Error "netstack_driver build failed"
    exit 1
}

Write-Output "=== Building kernel WITH network_stack ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "network_stack")
if ($exitCode -ne 0) {
    Write-Error "Kernel build (network_stack) failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (Test-Path $SerialLog) { Clear-Content $SerialLog }
$PcapFile = "_evidence\latest\multi-nic.pcap"
$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-netdev", "user,id=net0",
    "-device", "e1000,netdev=net0",
    "-object", "filter-dump,id=f0,netdev=net0,file=$PcapFile",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

Write-Output "=== Rebuilding default (non-feature) kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    "NETSTACK_FOUND",
    "DNS_CAPABILITY_RESOLVE_PASS",
    "DNS_CAPABILITY_STRANGER_DENIED_OK",
    "ROUTE_RESOLVED dest=127.0.0.1 iface=0",
    "ROUTE_RESOLVED dest=10.0.2.15 iface=1",
    "ROUTE_RESOLVED dest=192.168.1.50 iface=2",
    "ROUTE_RESOLVED dest=8.8.8.8 iface=1 next_hop=10.0.2.2",
    "MULTI_NIC_LPM_PASS",
    "LOOPBACK_PACKET_PASS",
    "NETSTACK_ICMP_SELF_CHECK_PASS",
    "NETSTACK_DNS_SELF_CHECK_PASS",
    "NETSTACK_TCP_SELF_CHECK_CONNECTED",
    "NETSTACK_HTTP_SELF_CHECK_PASS"
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
    Write-Error "Multi-NIC & DNS Capability test FAILED -- check log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "=== Multi-NIC Routing, Loopback & DNS Capability Wiring VERIFIED (Phase 10 Deliverables 3 & 5 PASS) ==="
