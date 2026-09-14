# Phase 12 (Item 12.1): VirtIO-GPU 2D Hardware Acceleration Test
# Boots QEMU with a modern VirtIO-GPU PCI device (-device virtio-vga)
# and verifies PCI capability negotiation, split-ring control virtqueue setup,
# and wire-format 2D commands (create, attach backing, scanout, transfer, flush).

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-virtio-gpu.log",
    [int]$BootWaitSeconds = 18
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

Write-Output "=== Building Kernel (VirtIO-GPU Hardware Acceleration) ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "Kernel build failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$evidenceDir = Split-Path $SerialLog -Parent
if (-not (Test-Path $evidenceDir)) {
    New-Item -ItemType Directory -Path $evidenceDir -Force | Out-Null
}
if (Test-Path $SerialLog) { Clear-Content $SerialLog }

Write-Output "=== Launching QEMU with VirtIO-VGA ==="
$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-device", "virtio-vga",
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

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$checks = @(
    "VIRTIO_GPU_FOUND",
    "VIRTIO_GPU_READY",
    "VIRTIO_GPU_RESOURCE_CREATE_PASS",
    "VIRTIO_GPU_ATTACH_BACKING_PASS",
    "VIRTIO_GPU_SET_SCANOUT_PASS",
    "VIRTIO_GPU_TRANSFER_PASS",
    "VIRTIO_GPU_FLUSH_PASS",
    "VIRTIO_GPU_2D_ACCEL_ACTIVE"
)

$allPassed = $true
foreach ($c in $checks) {
    if ($content.Contains($c)) {
        Write-Output "  PASS: $c"
    } else {
        Write-Warning "  FAIL: missing $c"
        $allPassed = $false
    }
}

if ($allPassed) {
    Write-Output "=== VIRTIO_GPU TEST: ALL CHECKS PASSED (100%) ==="
    exit 0
} else {
    Write-Error "=== VIRTIO_GPU TEST: FAILED ==="
    exit 1
}
