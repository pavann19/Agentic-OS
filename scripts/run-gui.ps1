# Interactive Hardware-Accelerated GUI Launcher for Agentic OS
# Runs QEMU with WHPX (Windows Hypervisor Platform) acceleration and native PS/2 Keyboard/Mouse!

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "D:\Operating_System\boot_rs\qemu_fatdir"
)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
[Environment]::CurrentDirectory = $repoRoot

$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

Write-Host "=== Agentic OS Hardware-Accelerated GUI Launcher ===" -ForegroundColor Cyan
Write-Host "Architecture: x86_64 Long Mode (64-bit UEFI)" -ForegroundColor Gray
Write-Host "Acceleration: WHPX (Windows Hypervisor Platform)" -ForegroundColor Gray
Write-Host "Input System: Native Intel 8042 PS/2 Keyboard + Mouse" -ForegroundColor Gray
Write-Host ""

# 1. Build terminal_emulator app
Write-Host "Building user_rs\terminal_emulator..." -ForegroundColor Yellow
$proc = Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\terminal_emulator" -PassThru -NoNewWindow -Wait
if ($proc.ExitCode -ne 0) {
    Write-Error "terminal_emulator build failed with code $($proc.ExitCode)"
    exit $proc.ExitCode
}

# 2. Build kernel WITH terminal_demo
Write-Host "Building kernel_rs with terminal_demo..." -ForegroundColor Yellow
$proc = Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none --features terminal_demo" -WorkingDirectory "$repoRoot\kernel_rs" -PassThru -NoNewWindow -Wait
if ($proc.ExitCode -ne 0) {
    Write-Error "Kernel build failed with code $($proc.ExitCode)"
    exit $proc.ExitCode
}

# 3. Deploy kernel to FAT dir
$kernelBin = "$repoRoot\kernel_rs\target\x86_64-unknown-none\release\agentic_kernel"
$destBin = "$FatDir\kernel.elf"
Copy-Item $kernelBin $destBin -Force
Write-Host "Kernel deployed to $destBin" -ForegroundColor Green

# 4. Launch QEMU with WHPX acceleration and interactive GUI display
Write-Host "`nLaunching QEMU interactive window with WHPX..." -ForegroundColor Cyan
Write-Host "Tip: Click inside the QEMU window to type and interact directly with the Terminal!" -ForegroundColor Magenta
Write-Host "Press Ctrl + Alt + G to release mouse grab if captured.`n" -ForegroundColor Gray

$qemuArgs = @(
    "-machine", "q35",
    "-accel", "whpx",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=$OvmfCode",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "stdio",
    "-no-reboot"
)

& $QemuExe @qemuArgs
