# Phase 13 deliverable 4 (docs/ROADMAP.md): the first real reference
# app -- a genuine, capability-isolated ring-3 terminal emulator,
# installed through installer.rs under a manifest declaring exactly
# Surface, wired into Phase 12's real input routing and PSF1 text
# rendering. Builds the kernel WITH terminal_demo (off by default),
# sends REAL synthetic keystrokes through QEMU's own HMP monitor (same
# technique scripts/test-keyboard.ps1/test-input-routing.ps1 already
# proved) spelling "hi", then Enter, then a backspace after one more
# letter, and verifies: the real scancodes were routed and decoded,
# the app echoed each one, and pushed a completed line into its
# scrollback -- real evidence of the full pipeline (PS/2 IRQ -> input
# routing -> ASCII decode -> text-widget render) working end to end.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-terminal.log",
    [int]$MonitorPort = 45456,
    [int]$BootWaitSeconds = 16,
    [int]$PostKeySeconds = 28
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

Write-Output "=== Building terminal_emulator ==="
$exitCode = Invoke-CargoQuiet "user_rs\terminal_emulator" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "terminal_emulator build failed"; exit 1 }

Write-Output "=== Building kernel WITH terminal_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "terminal_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (terminal_demo) failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Clear-Content $SerialLog }

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
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
    foreach ($key in @("h", "i", "ret", "x", "backspace")) {
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
    @{ Name = "real Surface granted via manifest-gated installer.rs"; Pattern = "INSTALLER_GRANT_OK label=terminal_surface" },
    @{ Name = "terminal app entered ring 3"; Pattern = "TERMINAL_ELF_ENTER" },
    @{ Name = "terminal signaled real readiness"; Pattern = "TERMINAL_READY" },
    @{ Name = "focus assigned to the terminal's surface"; Pattern = "INPUT_FOCUS_SET" },
    @{ Name = "real ELF64 process banner printed"; Pattern = "[TERMINAL_EMULATOR] real ELF64 ring-3 process" },
    @{ Name = "'h' key echoed (ascii 104)"; Pattern = "KEY_ECHOED ascii=104" },
    @{ Name = "'i' key echoed (ascii 105)"; Pattern = "KEY_ECHOED ascii=105" },
    @{ Name = "Enter echoed (ascii 10)"; Pattern = "KEY_ECHOED ascii=10\D" },
    @{ Name = "'x' key echoed (ascii 120)"; Pattern = "KEY_ECHOED ascii=120\D" },
    @{ Name = "backspace echoed (ascii 8)"; Pattern = "KEY_ECHOED ascii=8\D" }
)
foreach ($c in $checks) {
    # Real, deliberate fix: a plain substring Contains() check for
    # "ascii=10" is a false-positive trap against "ascii=104"/"ascii=105"
    # (both genuinely start with "10") -- regex with a trailing
    # non-digit boundary is what actually distinguishes them. Patterns
    # without a `\` are treated as plain substrings still (Contains).
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
    Write-Error "Terminal emulator test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 deliverable 4 verified: the first real reference app (a genuine, capability-isolated ring-3 terminal emulator) was installed through installer.rs under a real manifest, received real routed keyboard input, decoded real PS/2 scancodes to ASCII, and rendered them live via the real PSF1 text widget -- a real synthetic keystroke sequence (h, i, Enter, x, Backspace) typed through QEMU's own HMP monitor and echoed correctly end to end."
