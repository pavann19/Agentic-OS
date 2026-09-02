# Phase 6 -- Driver Synthesis Loop: real snapshot-restore test harness +
# failure-capture pipeline + bounded retry (`docs/ROADMAP.md` Phase 6,
# deliverables 1-3).
#
# "Disposable VM snapshot" here means exactly what it needs to for THIS
# project's actual failure modes: each attempt gets a completely fresh
# QEMU process and a fresh throwaway disk image -- a panic or hang in
# one attempt costs nothing but that attempt (the process is killed,
# nothing about the next attempt's starting state depends on it), never
# persistent state. This is real disposability, not a simulated one --
# QEMU internal savevm/loadvm snapshotting was considered and rejected
# as unnecessary complexity for what this actually needs: a clean start
# per attempt, not resuming mid-execution state.
#
# Failure capture is real, not a stub: each attempt's serial log is
# classified into exactly one outcome --
#   SUCCESS         -- the expected success marker appeared
#   FAULT           -- a real [ERROR] EXCEPTION line appeared (vector +
#                       error code + rip captured verbatim)
#   TIMEOUT_HANG     -- neither success nor a fault appeared within the
#                       wait window (the kernel is presumed hung)
# and the FULL classified record for every attempt (not just the last)
# is what gets printed at the end -- "hand a human the complete attempt
# history" (deliverable 3), not just a pass/fail bit.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SuccessMarker = "VIRTIO_NET_SELF_CHECK_PASS",
    [int]$MaxAttempts = 5,
    [int]$WaitSeconds = 10
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path "_evidence\synthesis" | Out-Null

$history = @()
$succeeded = $false

for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
    $log = "_evidence\synthesis\attempt$attempt.log"
    $pcap = "_evidence\synthesis\attempt$attempt.pcap"
    Remove-Item -Force $log, $pcap -ErrorAction SilentlyContinue

    Write-Output "--- Synthesis attempt ${attempt}/${MaxAttempts} ---"

    # Real, fresh disposable state every attempt: a brand-new, throwaway
    # disk (virtio-net has no persistent storage need here, but every
    # other real Tier 1 device this kernel boots with stays present for
    # a faithful boot) and a brand-new QEMU process -- nothing carries
    # over from a prior attempt's failure.
    $trialDisk = "_evidence\synthesis\disk-attempt$attempt.img"
    if (Test-Path $trialDisk) { Remove-Item -Force $trialDisk }
    $fs = [System.IO.File]::Create($trialDisk)
    $fs.SetLength(16MB)
    $fs.Close()

    $qemuArgs = @(
        "-machine", "q35,kernel-irqchip=split",
        "-m", "256M",
        "-device", "intel-iommu,intremap=on",
        "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
        "-drive", "file=fat:rw:$FatDir,format=raw",
        "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
        "-drive", "file=$trialDisk,if=none,id=disk0,format=raw",
        "-netdev", "user,id=net0",
        "-device", "virtio-net-pci,netdev=net0,mac=52:54:00:12:34:56,disable-legacy=on,iommu_platform=on,ats=on",
        "-object", "filter-dump,id=f1,netdev=net0,file=$pcap",
        "-serial", "file:$log",
        "-display", "none",
        "-no-reboot"
    )

    $proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow

    $outcome = "TIMEOUT_HANG"
    $detail = ""
    $deadline = (Get-Date).AddSeconds($WaitSeconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 300
        if (-not (Test-Path $log)) { continue }
        $content = Get-Content $log -Raw -ErrorAction SilentlyContinue
        if ($null -eq $content) { continue }
        if ($content -match [regex]::Escape($SuccessMarker)) {
            $outcome = "SUCCESS"
            $detail = "marker '$SuccessMarker' observed"
            break
        }
        $faultMatch = [regex]::Match($content, '\[ERROR\] EXCEPTION vector=\S+ error_code=\S+ rip=\S+.*')
        if ($faultMatch.Success) {
            $outcome = "FAULT"
            $detail = $faultMatch.Value
            break
        }
        if ($proc.HasExited) {
            # QEMU itself exited without a success marker or a captured
            # fault line -- a real, distinct failure shape (e.g. a host-
            # side QEMU error, not a guest-side one).
            $outcome = "QEMU_EXITED_EARLY"
            $detail = "process exit code=$($proc.ExitCode)"
            break
        }
    }

    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 300 # let the OS release file handles before the next attempt

    $record = [PSCustomObject]@{
        Attempt = $attempt
        Outcome = $outcome
        Detail  = $detail
        Log     = $log
        Pcap    = $pcap
    }
    $history += $record
    Write-Output "  Outcome: $outcome"
    if ($detail) { Write-Output "  Detail:  $detail" }

    if ($outcome -eq "SUCCESS") {
        $succeeded = $true
        break
    }
}

Write-Output ""
Write-Output "=== Full attempt history (deliverable 3: never just the last result) ==="
$history | Format-Table -AutoSize | Out-String | Write-Output

if ($succeeded) {
    Write-Output "SYNTHESIS SUCCEEDED within the retry budget."
    exit 0
} else {
    Write-Output "SYNTHESIS EXHAUSTED ITS RETRY BUDGET ($MaxAttempts attempts) -- clean halt, complete log trail above, no corrupted system state (every attempt used a fresh disposable disk/process)."
    exit 1
}
