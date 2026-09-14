# Interactive GUI Launcher for Agentic OS.
# Real, deliberate choice, not an oversight: this always uses TCG (software
# emulation), never WHPX -- every mode here attaches a real intel-iommu
# device (and launcher/files/net also attach virtio-blk-pci with
# iommu_platform=on/ats=on), which needs kernel-irqchip=split. WHPX does not
# support split irqchip (see kernel_rs's own WHPX work/docs for the exact
# incompatibility), so this script cannot use it without dropping IOMMU
# device assignment. For a fast, WHPX-accelerated single-app GUI session
# with no disk/network device (terminal or editor content only, no IOMMU
# needed), use `make run-qemu` with the matching *_demo kernel feature
# staged instead -- see scripts/run-manual.ps1.

param(
    [ValidateSet("launcher", "terminal", "editor", "files", "net")]
    [string]$Mode = "launcher",
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

Write-Host "=== Agentic OS GUI Launcher ===" -ForegroundColor Cyan
Write-Host "Mode:         $Mode" -ForegroundColor Green
Write-Host "Architecture: x86_64 Long Mode (64-bit UEFI)" -ForegroundColor Gray
Write-Host "Acceleration: TCG (required for IOMMU/virtio-blk device assignment; WHPX does not support kernel-irqchip=split)" -ForegroundColor Gray
Write-Host "Input System: Native Intel 8042 PS/2 Keyboard + Mouse" -ForegroundColor Gray
Write-Host ""

# 1. Build Required Apps & Kernel based on Mode
switch ($Mode) {
    "launcher" {
        Write-Host "Building all reference apps for Application Launcher (2x2 Desktop Grid)..." -ForegroundColor Yellow
        $apps = @(
            "user_rs\terminal_emulator",
            "user_rs\text_editor",
            "user_rs\virtio_blk_driver",
            "user_rs\file_manager",
            "user_rs\netstack_driver",
            "user_rs\net_client"
        )
        foreach ($app in $apps) {
            $proc = Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\$app" -PassThru -NoNewWindow -Wait
            if ($proc.ExitCode -ne 0) { Write-Error "$app build failed"; exit $proc.ExitCode }
        }
        Write-Host "Building kernel_rs with phase13_all..." -ForegroundColor Yellow
        $proc = Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none --features phase13_all" -WorkingDirectory "$repoRoot\kernel_rs" -PassThru -NoNewWindow -Wait
        if ($proc.ExitCode -ne 0) { Write-Error "Kernel build failed"; exit $proc.ExitCode }
    }
    "terminal" {
        Write-Host "Building terminal_emulator and kernel_rs (terminal_demo)..." -ForegroundColor Yellow
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\terminal_emulator" -NoNewWindow -Wait
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none --features terminal_demo" -WorkingDirectory "$repoRoot\kernel_rs" -NoNewWindow -Wait
    }
    "editor" {
        Write-Host "Building text_editor and kernel_rs (text_editor_demo)..." -ForegroundColor Yellow
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\text_editor" -NoNewWindow -Wait
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none --features text_editor_demo" -WorkingDirectory "$repoRoot\kernel_rs" -NoNewWindow -Wait
    }
    "files" {
        Write-Host "Building file_manager, virtio_blk_driver and kernel_rs..." -ForegroundColor Yellow
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\virtio_blk_driver" -NoNewWindow -Wait
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\file_manager" -NoNewWindow -Wait
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none --features file_manager_demo" -WorkingDirectory "$repoRoot\kernel_rs" -NoNewWindow -Wait
    }
    "net" {
        Write-Host "Building net_client, netstack_driver and kernel_rs..." -ForegroundColor Yellow
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\netstack_driver" -NoNewWindow -Wait
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none" -WorkingDirectory "$repoRoot\user_rs\net_client" -NoNewWindow -Wait
        Start-Process cargo -ArgumentList "build --release --target x86_64-unknown-none --features net_client_demo" -WorkingDirectory "$repoRoot\kernel_rs" -NoNewWindow -Wait
    }
}

# 2. Deploy kernel to FAT dir
$kernelBin = "$repoRoot\kernel_rs\target\x86_64-unknown-none\release\agentic_kernel"
$destBin = "$FatDir\kernel.elf"
Copy-Item $kernelBin $destBin -Force
Write-Host "Kernel deployed to $destBin" -ForegroundColor Green

# 3. Create fresh disk image for storage apps if in launcher or files mode
$diskImg = "$repoRoot\_evidence\disk-gui.img"
if ($Mode -eq "launcher" -or $Mode -eq "files") {
    $fs = [System.IO.File]::Create($diskImg)
    $fs.SetLength(8MB)
    $fs.Close()
}

# 4. Set repo-local temp directory for QEMU vvfat
$repoTemp = "$repoRoot\_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

# 5. Launch QEMU with interactive SDL GUI display
Write-Host "`nLaunching QEMU interactive window..." -ForegroundColor Cyan
Write-Host "Display:      SDL Native Desktop Window" -ForegroundColor Gray
Write-Host "Acceleration: TCG (split kernel-irqchip, needed for real IOMMU device assignment)" -ForegroundColor Gray
Write-Host "Tip: Click inside the QEMU window to type and interact directly with the Desktop!" -ForegroundColor Magenta
Write-Host "Press Ctrl + Alt + G to release mouse grab if captured.`n" -ForegroundColor Gray

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-vga", "std",
    "-display", "sdl",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=$OvmfCode",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-name", "Agentic-OS-GUI"
)

if ($Mode -eq "launcher" -or $Mode -eq "files") {
    $qemuArgs += @(
        "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
        "-drive", "file=$diskImg,if=none,id=disk0,format=raw"
    )
}

if ($Mode -eq "launcher" -or $Mode -eq "net") {
    $qemuArgs += @(
        "-netdev", "user,id=net0",
        "-device", "e1000,netdev=net0"
    )
}

$qemuArgs += @(
    "-serial", "stdio",
    "-no-reboot"
)

& $QemuExe @qemuArgs

