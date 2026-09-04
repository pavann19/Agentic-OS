# Phase 10 (docs/ROADMAP.md Sec5): real network-stack process.
# Builds the kernel WITH the network_stack feature (off by default --
# see kernel_rs/Cargo.toml's own comment), which makes netstack.rs own
# the real e1000-class NIC instead of e1000.rs's own Phase 8 demo, and
# boots it against QEMU's real user-mode networking (10.0.2.0/24, real
# gateway at 10.0.2.2). Checks for a real, live ICMP echo reply from
# that real gateway -- the first real, end-to-end, independently-
# meaningful milestone above the link layer (Ethernet RX, ARP, IPv4,
# ICMP all have to be real and correct for this to pass; a single wrong
# checksum or a byte-swapped field fails it, same as e1000_driver's own
# pcap-verified ARP self-check already proved for TX alone).
# Rebuilds the default (non-feature) kernel afterward, same discipline
# as every other feature-gated test script in this project.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-network.log",
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
$PcapFile = "_evidence\latest\netstack.pcap"
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
    "NETSTACK_IOMMU_DOMAIN_ASSIGNED",
    "device live, MAC=",
    "pinging real gateway 10.0.2.2",
    "NETSTACK_ICMP_SELF_CHECK_PASS"
    # DNS/UDP self-check is real, compiled code but currently disabled
    # at its one call site -- a genuine, disclosed, unresolved runtime
    # fault found this session (see netstack_driver's own main.rs
    # comment at that call site for the full investigation). Not
    # checked here until that's resolved.
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
    Write-Error "Network stack ICMP self-check FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Network stack verified: real Ethernet RX + ARP resolve + IPv4 + ICMP echo request/reply against QEMU's real gateway, independently captured in $PcapFile."
