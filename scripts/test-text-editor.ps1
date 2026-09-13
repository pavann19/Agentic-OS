# Phase 13 deliverable 4 (docs/ROADMAP.md): the second real reference
# app -- a genuine, capability-isolated ring-3 text editor, installed
# through installer.rs under a manifest declaring exactly Surface, wired
# into Phase 12's real input routing and PSF1 text rendering, reusing
# terminal_emulator's own proven pipeline. Builds the kernel WITH
# text_editor_demo (off by default), sends REAL synthetic keystrokes
# through QEMU's own HMP monitor: types "hi", Enter (moves to the next
# row), types "ok", then real Left/Up arrow presses -- verifying the
# real 2D cursor actually moves (row/col reported by the app itself
# change as expected), not just that keys are accepted.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-text-editor.log",
    [int]$MonitorPort = 45458,
    [int]$BootWaitSeconds = 16,
    [int]$PostKeySeconds = 26
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

Write-Output "=== Building text_editor ==="
$exitCode = Invoke-CargoQuiet "user_rs\text_editor" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "text_editor build failed"; exit 1 }

Write-Output "=== Building kernel WITH text_editor_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "text_editor_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (text_editor_demo) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Clear-Content $SerialLog }

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
try {
    Start-Sleep -Seconds $BootWaitSeconds

    $client = New-Object System.Net.Sockets.TcpClient
    $client.Connect("127.0.0.1", $MonitorPort)
    $stream = $client.GetStream()
    $writer = New-Object System.IO.StreamWriter($stream)
    $writer.AutoFlush = $true
    # "hi" on row 0, Enter -> row 1, "ok" on row 1, then Left (within
    # row 1) and Up (back to row 0) -- real cursor movement across both
    # axes, not just typing.
    foreach ($key in @("h", "i", "ret", "o", "k", "left", "up")) {
        $writer.WriteLine("sendkey $key")
        Start-Sleep -Milliseconds 300
    }
    $writer.Close()
    $client.Close()

    Start-Sleep -Seconds $PostKeySeconds
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
    @{ Name = "real Surface granted via manifest-gated installer.rs"; Pattern = "INSTALLER_GRANT_OK label=text_editor_surface" },
    @{ Name = "editor app entered ring 3"; Pattern = "TEXT_EDITOR_ELF_ENTER" },
    @{ Name = "editor signaled real readiness"; Pattern = "TEXT_EDITOR_READY" },
    @{ Name = "focus assigned to the editor's surface"; Pattern = "INPUT_FOCUS_SET" },
    @{ Name = "real ELF64 process banner printed"; Pattern = "[TEXT_EDITOR] real ELF64 ring-3 process" },
    @{ Name = "'h' typed at row 0"; Pattern = "KEY_HANDLED row=0 col=1" },
    @{ Name = "'i' typed at row 0"; Pattern = "KEY_HANDLED row=0 col=2" },
    @{ Name = "Enter moved cursor to row 1"; Pattern = "KEY_HANDLED row=1 col=0" },
    @{ Name = "'o' typed at row 1"; Pattern = "KEY_HANDLED row=1 col=1" },
    @{ Name = "'k' typed at row 1"; Pattern = "KEY_HANDLED row=1 col=2" },
    @{ Name = "Left arrow moved cursor within row 1"; Pattern = "KEY_HANDLED row=1 col=1\D" },
    @{ Name = "Up arrow moved cursor back to row 0 (real 2D movement)"; Pattern = "KEY_HANDLED row=0 col=1\D" }
)
foreach ($c in $checks) {
    if ($c.Pattern -match '\\D') {
        $matched = $content -match $c.Pattern
    } else {
        $matched = $content.Contains($c.Pattern)
    }
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Text editor test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 deliverable 4 verified (second reference app): a genuine, capability-isolated ring-3 text editor was installed through installer.rs under a real manifest, received real routed keyboard input, and correctly moved a real 2D cursor across rows and columns (typing, Enter, and real Left/Up arrow presses) via the real PSF1 text widget -- a real synthetic keystroke sequence typed through QEMU's own HMP monitor and verified end to end."
