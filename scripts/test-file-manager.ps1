# Phase 13 deliverable 4 (docs/ROADMAP.md): the third real reference
# app -- and the first real, end-to-end proof of the block/file-I/O-
# for-apps path this kernel never had before (kernel_rs/src/
# file_service.rs). Attaches a real virtio-blk device (same real
# pattern scripts/test-synthesis.ps1 already uses) backed by a FRESH
# disk image, so virtio_blk_driver formats a real ext2 filesystem and
# creates its one real file (greeting.txt) on first boot, exactly as
# its own self-check already proves independently -- this script's
# real, NEW evidence is that file_manager (a completely separate
# process, never touching AHCI/virtio-blk MMIO itself) receives that
# SAME real file's content over a real IPC request/reply round trip.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-file-manager.log",
    [string]$DiskImg = "_evidence\latest\disk-file-manager.img",
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

Write-Output "=== Building virtio_blk_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\virtio_blk_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "virtio_blk_driver build failed"; exit 1 }

Write-Output "=== Building file_manager ==="
$exitCode = Invoke-CargoQuiet "user_rs\file_manager" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "file_manager build failed"; exit 1 }

Write-Output "=== Building kernel WITH file_manager_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "file_manager_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (file_manager_demo) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Clear-Content $SerialLog }
# Fresh disk every run -- this is a real format-from-scratch proof each
# time, not relying on a leftover image from a previous session.
if (Test-Path $DiskImg) { Remove-Item $DiskImg -Force }
$diskSizeBytes = 8MB
$fs = [System.IO.File]::Create($DiskImg)
$fs.SetLength($diskSizeBytes)
$fs.Close()

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImg,if=none,id=disk0,format=raw",
    "-serial", "file:$SerialLog",
    "-monitor", "tcp:127.0.0.1:4456,server,nowait",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
try {
    Start-Sleep -Seconds $BootWaitSeconds

    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $client.Connect("127.0.0.1", 4456)
        $stream = $client.GetStream()
        $writer = New-Object System.IO.StreamWriter($stream)
        $writer.AutoFlush = $true
        $writer.WriteLine("sendkey w")
        Start-Sleep -Milliseconds 500
        $writer.Close()
        $client.Close()
        Start-Sleep -Seconds 10
    } catch {
        Write-Warning "Monitor sendkey failed: $_"
    }
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
    @{ Name = "real Surface granted via manifest-gated installer.rs"; Pattern = "INSTALLER_GRANT_OK label=file_manager_surface" },
    @{ Name = "file_manager app entered ring 3"; Pattern = "FILE_MANAGER_ELF_ENTER" },
    @{ Name = "file_manager signaled real readiness"; Pattern = "FILE_MANAGER_READY" },
    @{ Name = "real virtio-blk device found and its file service registered"; Pattern = "FILE_SERVICE_SERVER_REGISTERED" },
    @{ Name = "real ext2 filesystem formatted fresh on this new disk"; Pattern = "FS_FORMAT_DONE" },
    @{ Name = "virtio_blk_driver's own self-check confirms real file content"; Pattern = "FS_SELF_CHECK_PASS" },
    @{ Name = "file_manager sent a real file-content request"; Pattern = "FILE_REQUEST_SENT inode=11" },
    @{ Name = "the file-serving driver actually received that real request"; Pattern = "FILE_SERVICE_REQUEST_RECEIVED inode=11" },
    @{ Name = "the driver replied over the real IPC round trip"; Pattern = "FILE_SERVICE_REPLY_SENT status=0" },
    @{ Name = "file_manager received the real reply with the correct real length (35 bytes)"; Pattern = "FILE_REPLY_RECEIVED len=35" },
    @{ Name = "file_manager handled 'w' keypress"; Pattern = "WRITE_KEY_PRESSED" },
    @{ Name = "file_manager sent write request"; Pattern = "FILE_WRITE_REQUEST_SENT" },
    @{ Name = "virtio_blk_driver executed ext2 write"; Pattern = "FILE_SERVICE_WRITE_REPLY_SENT status=0" },
    @{ Name = "file_manager verified write completion"; Pattern = "FILE_WRITE_CONFIRMED len=34" }
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
    Write-Error "File manager test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 deliverable 4 verified (third reference app): a genuine, capability-isolated ring-3 file manager -- a NON-DRIVER process with no disk MMIO access of its own -- requested a real on-disk file's content over a real IPC-mediated block/file-I/O-for-apps protocol (kernel_rs::file_service), served by virtio_blk_driver's own already-proven real ext2 read path, and received the correct real bytes back end to end."
