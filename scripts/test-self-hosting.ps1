# Self-Hosting Groundwork Verification Script (Milestone 2)
# docs/TASK_SELF_HOSTING_GROUNDWORK.md: Live end-to-end test of:
#  1. Directory-based filesystem (mkdir, readdir, multi-file path resolution over ext2).
#  2. Generic on-demand process exec syscall (spawn child ELF binary by path from running ring-3 process, waitpid exit code 42).

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-self-hosting.log",
    [string]$DiskImg = "_evidence\latest\disk-self-hosting.img",
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

Write-Output "=== Building child_proc ELF ==="
$exitCode = Invoke-CargoQuiet "user_rs\child_proc" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "child_proc build failed"; exit 1 }

Write-Output "=== Building virtio_blk_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\virtio_blk_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "virtio_blk_driver build failed"; exit 1 }

Write-Output "=== Building netstack_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "netstack_driver build failed"; exit 1 }

Write-Output "=== Building net_client ==="
$exitCode = Invoke-CargoQuiet "user_rs\net_client" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "net_client build failed"; exit 1 }

Write-Output "=== Building kernel WITH net_client_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "net_client_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }
if (Test-Path $DiskImg) { Remove-Item $DiskImg -Force }

# Fresh 8MB disk image for ext2 filesystem
$diskSizeBytes = 8MB
$fs = [System.IO.File]::Create($DiskImg)
$fs.SetLength($diskSizeBytes)
$fs.Close()

Write-Output "=== Launching QEMU (Self-Hosting Groundwork Run) ==="
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

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Name = "virtio-blk file service registered"; Pattern = "FILE_SERVICE_SERVER_REGISTERED" },
    @{ Name = "net_client entered ring 3"; Pattern = "NET_CLIENT_ELF_ENTER" },
    @{ Name = "Groundwork verification started"; Pattern = "[SELF_HOSTING] Groundwork Verification Starting" },
    @{ Name = "Created /src directory"; Pattern = "[SELF_HOSTING] MKDIR /src OK" },
    @{ Name = "Created /src/bin subdirectory"; Pattern = "[SELF_HOSTING] MKDIR /src/bin OK" },
    @{ Name = "Directory creation confirmed"; Pattern = "[SELF_HOSTING] MKDIR_SUCCESS" },
    @{ Name = "Wrote file by multi-file path"; Pattern = "[SELF_HOSTING] WRITE_PATH OK" },
    @{ Name = "Read back file by path & verified content"; Pattern = "[SELF_HOSTING] READ_PATH OK: byte-for-byte content matched" },
    @{ Name = "Write/read path verified"; Pattern = "[SELF_HOSTING] WRITE_READ_PATH_SUCCESS" },
    @{ Name = "Readdir found directory entries"; Pattern = "[SELF_HOSTING] READDIR /src/bin found count=" },
    @{ Name = "Readdir listed hello.txt"; Pattern = "hello.txt" },
    @{ Name = "Readdir operation verified"; Pattern = "[SELF_HOSTING] READDIR_SUCCESS" },
    @{ Name = "Unlinked file /src/bin/hello.txt"; Pattern = "[SELF_HOSTING] UNLINK OK" },
    @{ Name = "Unlink operation verified"; Pattern = "[SELF_HOSTING] UNLINK_SUCCESS" },
    @{ Name = "Spawned child process on-demand"; Pattern = "[SELF_HOSTING] SPAWN_SUCCESS pid=" },
    @{ Name = "Child process started in ring 3"; Pattern = "[CHILD_PROC] spawned on-demand in ring 3" },
    @{ Name = "Child executed and exited with 42"; Pattern = "[CHILD_PROC] executing child task and exiting with code 42" },
    @{ Name = "Waitpid received exit code 42"; Pattern = "[SELF_HOSTING] WAITPID_SUCCESS exit_code=42" },
    @{ Name = "All Milestone 2 criteria verified"; Pattern = "[SELF_HOSTING] ALL_MILESTONE_2_VERIFIED" }
)

Write-Output "--- Self-Hosting Groundwork Verification ---"
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
    Write-Error "test-self-hosting FAILED"
    exit 1
}

Write-Output ""
Write-Output "=== ALL SELF-HOSTING GROUNDWORK (MILESTONE 2) TESTS PASSED ==="
