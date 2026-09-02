# Phase 7 -- real, end-to-end verification of the text shell
# (`docs/ROADMAP.md` Sec5 Phase 7). Every other test script in this
# project routes COM1 to a FILE (`-serial file:...`) since it only ever
# needs to READ what the kernel/drivers wrote. This script is the first
# that needs REAL bidirectional interaction -- typed commands going IN,
# not just log output coming out -- so it routes COM1 through a real
# TCP chardev socket instead (`-chardev socket ... -serial chardev:...`)
# and drives it like an actual serial terminal would: connect, read the
# boot banner, send real command lines, read real responses.
#
# What this proves, concretely:
#   - `ps`/`tools`/`audit` are real, typed, capability-gated Phase 5
#     syscalls reached through a human-typed command line, not a
#     separate privileged path -- the shell's own module doc states
#     this; this script is the live proof.
#   - `rawin 64` (port 0x64, the PS/2 controller -- NOT COM1, the only
#     port this shell process was ever granted) demonstrates the real,
#     hardware-enforced capability boundary: this process holds no more
#     privilege than any other capability-gated ring-3 process, and the
#     real TSS IOPB + this kernel's own ring-3 fault isolation end THIS
#     PROCESS for the attempt -- not a graceful in-band "permission
#     denied" this code prints, the actual documented behavior.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$DiskImage = "_evidence\disk-shell-test.img",
    [int]$ComPort = 45500,
    [int]$BootWaitSeconds = 9,
    [int]$CommandWaitMs = 2000
)

$ErrorActionPreference = "Stop"

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $DiskImage) | Out-Null
if (-not (Test-Path $DiskImage)) {
    $fs = [System.IO.File]::Create($DiskImage)
    $fs.SetLength(16MB)
    $fs.Close()
}

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-chardev", "socket,id=shellcom,host=127.0.0.1,port=$ComPort,server,nowait",
    "-serial", "chardev:shellcom",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow

$transcript = New-Object System.Text.StringBuilder

try {
    # Real fix for a genuine timing race this script used to have:
    # QEMU's "server,nowait" chardev socket does NOT buffer serial
    # output for a client that hasn't connected yet -- connecting only
    # after a fixed wall-clock delay risks missing everything printed
    # before that delay elapsed (the shell's own banner included),
    # non-deterministically, depending on real host/TCG timing
    # variance. Fixed by connecting as early as possible instead
    # (retrying until QEMU's listener is actually up, typically well
    # under a second), so nothing after that point can be missed
    # regardless of how the boot sequence's own timing varies.
    $client = $null
    $connectDeadline = (Get-Date).AddSeconds(10)
    while ((Get-Date) -lt $connectDeadline) {
        try {
            $client = New-Object System.Net.Sockets.TcpClient
            $client.Connect("127.0.0.1", $ComPort)
            break
        } catch {
            $client = $null
            Start-Sleep -Milliseconds 200
        }
    }
    if ($null -eq $client) {
        throw "could not connect to QEMU's chardev socket on port $ComPort"
    }
    $stream = $client.GetStream()
    $stream.ReadTimeout = 2000

    function Read-Available {
        $buf = New-Object byte[] 8192
        $total = ""
        try {
            while ($stream.DataAvailable -or $total -eq "") {
                if (-not $stream.DataAvailable) { Start-Sleep -Milliseconds 100; if (-not $stream.DataAvailable) { break } }
                $n = $stream.Read($buf, 0, $buf.Length)
                if ($n -le 0) { break }
                $total += [System.Text.Encoding]::ASCII.GetString($buf, 0, $n)
            }
        } catch [System.IO.IOException] { }
        return $total
    }

    function Send-Line($text) {
        $bytes = [System.Text.Encoding]::ASCII.GetBytes("$text`r`n")
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush()
    }

    # Now connected before boot output begins -- drain until the
    # shell's own prompt has genuinely appeared (real content-based
    # wait, not a guessed delay), bounded so a real hang still fails
    # cleanly rather than looping forever.
    $bootDeadline = (Get-Date).AddSeconds($BootWaitSeconds)
    while ((Get-Date) -lt $bootDeadline) {
        [void]$transcript.Append((Read-Available))
        if ($transcript.ToString() -match "agentos> ") { break }
        Start-Sleep -Milliseconds 200
    }

    foreach ($cmd in @("help", "ps", "tools", "audit")) {
        Send-Line $cmd
        Start-Sleep -Milliseconds $CommandWaitMs
        [void]$transcript.Append((Read-Available))
    }

    # Real natural-language intent path (deliverable 4): the SAME
    # underlying command, reached via a free-text sentence instead of
    # the exact command name.
    Send-Line "show me the processes"
    Start-Sleep -Milliseconds $CommandWaitMs
    [void]$transcript.Append((Read-Available))

    # The real capability-boundary demo: port 0x64 (PS/2 controller) is
    # NOT in this shell's COM1-only PortIoRange grant. Real bug found
    # and fixed via this exact test (see gdt.rs's set_iopb doc comment):
    # the IOPB used to be a single GLOBAL TSS field, so a port granted
    # to ANY earlier driver (keyboard_driver's own 0x60-0x64 grant)
    # leaked to every later ring-3 process, including this shell --
    # `rawin 64` originally SUCCEEDED when it should have faulted. Fixed
    # by giving every thread its own IOPB copy, reloaded into the one
    # live TSS on every scheduler switch.
    #
    # Marker-based check, not a plain substring match: PROCESS_KILLED
    # ALSO appears earlier in every normal boot (fault_isolation_demo's
    # own, unrelated, deliberate crash) -- a naive match against the
    # whole transcript would find that pre-existing occurrence and
    # falsely "confirm" this check even if rawin's own attempt never
    # even reached the shell. Count occurrences before/after instead.
    $killCountBefore = ([regex]::Matches($transcript.ToString(), [regex]::Escape("PROCESS_KILLED"))).Count
    Send-Line "rawin 64"
    Start-Sleep -Milliseconds $CommandWaitMs
    $rawinOutput = Read-Available
    [void]$transcript.Append($rawinOutput)
    $killCountAfter = ([regex]::Matches($transcript.ToString(), [regex]::Escape("PROCESS_KILLED"))).Count
    $newKillObserved = $killCountAfter -gt $killCountBefore

    $client.Close()
} finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

$content = $transcript.ToString()
New-Item -ItemType Directory -Force -Path "_evidence\latest" | Out-Null
Set-Content -Path "_evidence\latest\shell-transcript.log" -Value $content

$checks = @(
    @{ Name = "shell banner";        Pattern = "Agentic OS shell" },
    @{ Name = "prompt appears";      Pattern = "agentos> " },
    @{ Name = "help output";         Pattern = "real typed thread list" },
    @{ Name = "ps output";           Pattern = "PID  STATE    USER" },
    @{ Name = "tools output";        Pattern = "SYSCALL  REQUIRED_RIGHTS" },
    @{ Name = "audit output ran (human-readable)"; Pattern = "Grant           object=|no audit records" },
    @{ Name = "natural-language intent resolved"; Pattern = [regex]::Escape("(interpreted as: ps)") }
)

$allPassed = $true
foreach ($c in $checks) {
    if ($content -match $c.Pattern) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# The capability-boundary case: EITHER a NEW kill genuinely happened
# right after rawin (real evidence, not a pre-existing occurrence
# elsewhere in the boot log) OR -- if this ever changes -- the raw
# value legitimately came back, which would mean this shell was
# WRONGLY granted that port, a real finding, not something to silently
# pass.
$rawinSucceeded = $rawinOutput -match "value = 0x"
if ($newKillObserved) {
    Write-Output "  PASS: capability boundary enforced -- rawin to an ungranted port ended THIS process (a NEW PROCESS_KILLED observed right after the attempt, not a pre-existing one), real IOPB/fault-isolation evidence"
} elseif ($rawinSucceeded) {
    Write-Output "  FAIL: rawin to an ungranted port SUCCEEDED -- this shell process was not actually confined to its COM1-only PortIoRange grant, a real capability-enforcement gap"
    $allPassed = $false
} else {
    Write-Output "  FAIL: neither a new kill nor a raw value was observed after rawin -- inconclusive, treat as a failure"
    $allPassed = $false
}

Write-Output ""
Write-Output "--- Full transcript saved to _evidence\latest\shell-transcript.log ---"

if ($allPassed) {
    Write-Output "Shell verified end-to-end: real typed commands over a real bidirectional serial line, routed through Phase 5's real capability-gated syscalls, with a real, hardware-enforced capability boundary demonstrated."
    exit 0
} else {
    Write-Error "One or more shell checks failed -- see output above and the saved transcript."
    exit 1
}
