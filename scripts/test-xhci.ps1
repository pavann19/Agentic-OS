# Phase 11 (docs/ROADMAP.md Sec5, deliverable 2): real xHCI (USB host
# controller) discovery. Boots the default kernel against a real
# QEMU-emulated `qemu-xhci` controller and checks for real Capability
# Register decoding (CAPLENGTH/HCIVERSION/HCSPARAMS1, real
# hardware-reported max device slots and max port count) -- the first
# real step toward USB support, same "spec plus config space,
# IOMMU-contained" discipline every driver since Phase 6 has used.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-xhci.log",
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

Write-Output "=== Building usb_xhci_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\usb_xhci_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "usb_xhci_driver build failed"
    exit 1
}

Write-Output "=== Building default kernel (usb_xhci is unconditional, like ahci/nvme) ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "Kernel build failed"
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
    "-device", "qemu-xhci,id=xhci0",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)
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
    "XHCI_FOUND",
    "XHCI_MAPPED",
    "XHCI_IOMMU_DOMAIN_ASSIGNED",
    "CAPLENGTH=0x",
    "XHCI_SELF_CHECK_PASS",
    "XHCI_RESET_PASS",
    "XHCI_COMMAND_PASS"
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
    Write-Error "xHCI self-check FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "xHCI (USB) host controller verified: real device found by PCI class code, real MMIO+IOMMU capability grants, real Capability Register set decoded from actual hardware, a real HCRST reset handshake, and one full real command round trip (a NO-OP command through a real Command Ring, completed via a real Event Ring Command Completion Event)."
