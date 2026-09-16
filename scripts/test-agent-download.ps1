# Agent-Driven Internet Download Verification Script
# TASK_AGENT_DOWNLOAD.md: Live end-to-end test of HTTP download -> ext2 file persistence
#
# Validates:
#  1. netstack_driver discovers e1000 NIC and registers net_service.
#  2. virtio_blk_driver registers file_service and formats/mounts ext2.
#  3. net_client enters ring 3 and sends real HTTP fetch request via net_service IPC.
#  4. netstack_driver fetches payload via real TCP and returns HTTP reply.
#  5. net_client extracts body and writes to ext2 filesystem via SYS_FILE_SERVICE_WRITE.
#  6. virtio_blk_driver commits multi-block file data to disk blocks and updates inode.
#  7. net_client reads back file from disk via SYS_FILE_SERVICE_REQUEST and verifies SHA-256 digest.
#  8. Adversarial check: unauthorized client lacking FileObject capability is denied and audited.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-agent-download.log",
    [string]$UnauthLog = "_evidence\latest\serial-agent-download-unauth.log",
    [string]$DiskImg = "_evidence\latest\disk-agent-download.img",
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

Write-Output "=== Building netstack_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\netstack_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "netstack_driver build failed"; exit 1 }

Write-Output "=== Building net_client ==="
$exitCode = Invoke-CargoQuiet "user_rs\net_client" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "net_client build failed"; exit 1 }

Write-Output "=== Building kernel WITH net_client_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "net_client_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (net_client_demo) failed"; exit 1 }
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

Write-Output "=== Launching QEMU (Authorized Download Run) ==="
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
    @{ Name = "net_service server registered by netstack"; Pattern = "NET_SERVICE_SERVER_REGISTERED" },
    @{ Name = "net_client entered ring 3"; Pattern = "NET_CLIENT_ELF_ENTER" },
    @{ Name = "net_client signaled readiness to kernel"; Pattern = "NET_CLIENT_READY" },
    @{ Name = "net_client started agent download"; Pattern = "AGENT_DOWNLOAD_START" },
    @{ Name = "net_client issued HTTP fetch request"; Pattern = "HTTP_FETCH_REQUEST_SENT" },
    @{ Name = "netstack_driver received IPC request"; Pattern = "NET_SERVICE_REQUEST_RECEIVED" },
    @{ Name = "netstack_driver sent reply via IPC"; Pattern = "NET_SERVICE_REPLY_SENT" },
    @{ Name = "net_client received HTTP response"; Pattern = "HTTP_RESPONSE_RECEIVED" },
    @{ Name = "net_client extracted HTTP body"; Pattern = "HTTP_BODY_EXTRACTED" },
    @{ Name = "net_client computed body SHA-256"; Pattern = "HTTP_BODY_SHA256=" },
    # Writes to a real, dedicated path ("/downloaded.dat") rather than
    # the shared literal inode 11 -- fixed this session (see
    # user_rs/net_client/src/main.rs's own comment) because that inode
    # was ALSO main.rs's Phase 4 object-store demo file and
    # virtio_blk_driver's own boot-time FS_SELF_CHECK target, so
    # whichever of the two ran last silently clobbered the other.
    @{ Name = "net_client initiated disk persistence"; Pattern = "PERSISTING_TO_DISK path=/downloaded.dat" },
    @{ Name = "virtio-blk committed file write to disk"; Pattern = "FILE_SERVICE_WRITE_COMMITTED" },
    @{ Name = "net_client confirmed file write"; Pattern = "FILE_WRITE_CONFIRMED" },
    @{ Name = "net_client initiated disk readback"; Pattern = "DISK_READBACK_START path=/downloaded.dat" },
    @{ Name = "net_client computed readback SHA-256"; Pattern = "DISK_READBACK_SHA256=" },
    @{ Name = "SHA-256 checksums byte-matched"; Pattern = "DOWNLOAD_CHECKSUM_VERIFIED: sha256 byte-matched" },
    @{ Name = "Agent-driven download succeeded end-to-end"; Pattern = "AGENT_DOWNLOAD_SUCCESS" }
)

Write-Output "--- Authorized Run Verification ---"
foreach ($c in $checks) {
    $matched = $content.Contains($c.Pattern)
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# Cross-validate that extracted body hash and readback hash match in PowerShell
$bodyHashMatch = [regex]::Match($content, "HTTP_BODY_SHA256=([0-9a-f]{64})")
$readHashMatch = [regex]::Match($content, "DISK_READBACK_SHA256=([0-9a-f]{64})")
if ($bodyHashMatch.Success -and $readHashMatch.Success) {
    $bHash = $bodyHashMatch.Groups[1].Value
    $rHash = $readHashMatch.Groups[1].Value
    Write-Output "  Extracted HTTP Body SHA-256 : $bHash"
    Write-Output "  Ext2 Disk Readback SHA-256  : $rHash"
    if ($bHash -eq $rHash) {
        Write-Output "  PASS: Independent host-side SHA-256 match validated"
    } else {
        Write-Output "  FAIL: Hash mismatch ($bHash != $rHash)"
        $allPassed = $false
    }
} else {
    Write-Output "  FAIL: Could not extract 64-char SHA-256 hex strings from serial log"
    $allPassed = $false
}

Write-Output ""
Write-Output "=== Running Adversarial Capability Denial Test ==="
Write-Output "=== Building kernel WITH agent_download_unauthorized ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "agent_download_unauthorized")
if ($exitCode -ne 0) { Write-Error "Kernel build (agent_download_unauthorized) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (Test-Path $UnauthLog) { Remove-Item $UnauthLog -Force }

$procUnauth = Start-Process -FilePath $QemuExe -ArgumentList @(
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
    "-serial", "file:$UnauthLog",
    "-display", "none",
    "-no-reboot"
) -PassThru -NoNewWindow

try {
    Start-Sleep -Seconds $BootWaitSeconds
} finally {
    if (-not $procUnauth.HasExited) {
        Stop-Process -Id $procUnauth.Id -Force -ErrorAction SilentlyContinue
    }
}

Write-Output "=== Restoring default kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $UnauthLog)) { Write-Error "No serial output at $UnauthLog"; exit 1 }
$unauthContent = Get-Content $UnauthLog -Raw

Write-Output "--- Adversarial Denial Verification ---"
# The literal "arg0=99" this second check used to assert on was an
# artifact of the OLD code path (a raw hardcoded capability id passed
# directly as the write syscall's arg0, unauthorized builds used the
# sentinel value 99 for it). The fix this session that moved the
# authorized path off the shared literal FILE_INODE (11) onto a real
# resolved-by-path inode (see the authorized-run checks above) means
# arg0 here is now whatever real inode FS_OP_LOOKUP/FS_OP_CREATE
# resolved for "/downloaded.dat" -- legitimately non-deterministic
# across runs depending on what else was allocated first, not a
# regression. What's still a real, checkable security property is that
# SOME concrete arg0 got denied, i.e. the denial fired on the real
# resolved capability check, not vacuously.
$unauthChecks = @(
    @{ Name = "Kernel denied unauthorized file write"; Pattern = "FILE_SERVICE_WRITE_DENIED: caller lacks FileObject WRITE capability" },
    @{ Name = "Denial was logged against a real, concrete arg0"; Regex = "FILE_SERVICE_WRITE_DENIED: caller lacks FileObject WRITE capability for arg0=\d+" },
    @{ Name = "net_client received write denial"; Pattern = "FILE_SERVICE_WRITE_DENIED_OR_NO_SERVER" }
)

foreach ($c in $unauthChecks) {
    $matched = if ($c.Regex) { $unauthContent -match $c.Regex } else { $unauthContent.Contains($c.Pattern) }
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if ($unauthContent.Contains("AGENT_DOWNLOAD_SUCCESS")) {
    Write-Output "  FAIL: Unauthorized client unexpectedly reported AGENT_DOWNLOAD_SUCCESS!"
    $allPassed = $false
} else {
    Write-Output "  PASS: Unauthorized client did not reach AGENT_DOWNLOAD_SUCCESS"
}

if (-not $allPassed) {
    Write-Error "test-agent-download FAILED"
    exit 1
}

Write-Output ""
Write-Output "=== ALL AGENT DOWNLOAD TESTS PASSED ==="
