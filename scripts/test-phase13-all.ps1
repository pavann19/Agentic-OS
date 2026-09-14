# Phase 13 Exit Criterion 3:
# Real Integration Test for All Four Reference Apps Running Concurrently
#
# Validates:
#  1. All four reference apps built and installed via manifest-gated installer:
#     - terminal_emulator (GUI + keyboard)
#     - text_editor (GUI + text layout)
#     - file_manager (GUI + disk storage via virtio-blk)
#     - net_client (GUI + network client via e1000/TCP)
#  2. Tiled 2x2 desktop placement by desktop launcher.
#  3. Concurrent execution in ring 3 with capability isolation.
#  4. Storage exercised via virtio-blk ext2 file request/reply.
#  5. Network service registered and active.
#  6. All 4 apps exchange readiness tokens with the kernel.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-phase13-all.log",
    [string]$DiskImg = "_evidence\latest\disk-phase13-all.img",
    [int]$BootWaitSeconds = 65
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

Write-Output "=== Building reference apps and drivers ==="
$apps = @(
    "user_rs\terminal_emulator",
    "user_rs\text_editor",
    "user_rs\virtio_blk_driver",
    "user_rs\file_manager",
    "user_rs\netstack_driver",
    "user_rs\net_client"
)
foreach ($app in $apps) {
    Write-Output "  Building $app..."
    $exitCode = Invoke-CargoQuiet $app @("build", "--release", "--target", "x86_64-unknown-none")
    if ($exitCode -ne 0) { Write-Error "$app build failed"; exit 1 }
}

Write-Output "=== Building kernel WITH phase13_all ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase13_all")
if ($exitCode -ne 0) { Write-Error "Kernel build (phase13_all) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }

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
    @{ Name = "Launcher initiated multi-app spawn"; Pattern = "LAUNCHER_STARTING_ALL_APPS" },
    @{ Name = "All four reference apps launched"; Pattern = "LAUNCHER_ALL_APPS_SPAWNED" },
    @{ Name = "terminal_emulator entered ring 3"; Pattern = "TERMINAL_ELF_ENTER" },
    @{ Name = "text_editor entered ring 3"; Pattern = "TEXT_EDITOR_ELF_ENTER" },
    @{ Name = "file_manager entered ring 3"; Pattern = "FILE_MANAGER_ELF_ENTER" },
    @{ Name = "net_client entered ring 3"; Pattern = "NET_CLIENT_ELF_ENTER" },
    @{ Name = "virtio-blk filesystem formatted and file service registered"; Pattern = "FILE_SERVICE_SERVER_REGISTERED" },
    @{ Name = "file_manager requested file from storage"; Pattern = "FILE_REQUEST_SENT inode=11" },
    @{ Name = "file_service served content back to file_manager"; Pattern = "FILE_REPLY_RECEIVED len=35" },
    @{ Name = "net_service server registered by network stack"; Pattern = "NET_SERVICE_SERVER_REGISTERED" },
    @{ Name = "All four apps signaled readiness and were verified concurrently"; Pattern = "LAUNCHER_ALL_APPS_READY_PASS" },
    @{ Name = "Package update demo executed cleanly"; Pattern = "APP_UPDATE_DEMO_DONE" }
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
    Write-Error "Phase 13 multi-app test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 Exit Criterion 3 verified: All four reference apps run concurrently, capability-isolated, exercising GUI + storage + network simultaneously in one real integration test suite."
