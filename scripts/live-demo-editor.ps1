# Live Keystroke Injection Demo for Agentic OS Text Editor
# Launches a hardware-accelerated GUI window and types live into the Text Editor!

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "D:\Operating_System\boot_rs\qemu_fatdir",
    [int]$MonitorPort = 4446,
    [int]$KeyDelayMs = 220
)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
[Environment]::CurrentDirectory = $repoRoot

$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

Write-Host "=== Agentic OS: Live Text Editor Keystroke Demo ===" -ForegroundColor Cyan
Write-Host "1. Building text_editor app and kernel..." -ForegroundColor Yellow

# Build text_editor
Push-Location "$repoRoot\user_rs\text_editor"
& cargo build --release --target x86_64-unknown-none | Out-Null
Pop-Location

# Build kernel with text_editor_demo
Push-Location "$repoRoot\kernel_rs"
& cargo build --release --target x86_64-unknown-none --features text_editor_demo | Out-Null
Pop-Location

# Deploy kernel to FAT dir
Copy-Item "$repoRoot\kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force
Write-Host "Kernel deployed successfully." -ForegroundColor Green

# Launch QEMU with visible GUI display and monitor
Write-Host "`n2. Launching graphical window on your screen..." -ForegroundColor Cyan
Write-Host "A desktop window will open displaying the Text Editor." -ForegroundColor Gray

$serialLog = "$repoRoot\_evidence\latest\serial-live-editor.log"
New-Item -ItemType Directory -Force -Path (Split-Path $serialLog) | Out-Null
if (Test-Path $serialLog) { Remove-Item $serialLog -Force }

$qemuArgs = @(
    "-machine", "q35",
    "-accel", "whpx",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$serialLog",
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait",
    "-no-reboot"
)

# Launch QEMU with WHPX, fallback to TCG if WHPX is unavailable
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru
Start-Sleep -Milliseconds 500
if ($proc.HasExited) {
    Write-Host "WHPX acceleration unavailable, switching to TCG interpreter..." -ForegroundColor Yellow
    $qemuArgs[1] = "q35,kernel-irqchip=split"
    $qemuArgs[3] = "tcg,tb-size=128"
    $proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru
}
Write-Host "QEMU running (PID: $($proc.Id)). Waiting for Text Editor to render..." -ForegroundColor Yellow

try {
    # Poll for TCP monitor readiness
    $connected = $false
    $retries = 30
    $client = $null
    while ($retries -gt 0 -and -not $connected) {
        Start-Sleep -Seconds 1
        try {
            $client = New-Object System.Net.Sockets.TcpClient
            $client.Connect("127.0.0.1", $MonitorPort)
            $connected = $true
        } catch {
            $retries--
        }
    }

    if (-not $connected) {
        Write-Error "Could not connect to QEMU monitor on port $MonitorPort"
        exit 1
    }

    # Wait for UEFI boot services exit and ring-3 text editor launch
    Write-Host "Waiting for Text Editor window and focus..." -ForegroundColor Yellow
    $booted = $false
    $timeoutSec = 30
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.Elapsed.TotalSeconds -lt $timeoutSec -and -not $booted) {
        Start-Sleep -Milliseconds 400
        if (Test-Path $serialLog) {
            $logText = Get-Content $serialLog -Raw -ErrorAction SilentlyContinue
            if ($logText -and ($logText.Contains("TEXT_EDITOR_READY") -or $logText.Contains("INPUT_FOCUS_SET"))) {
                $booted = $true
                break
            }
        }
    }
    Start-Sleep -Milliseconds 500

    Write-Host "`n>>> Text Editor ready! Beginning live keystroke injection now... <<<" -ForegroundColor Green
    Write-Host "Watch your QEMU window -- typing live into Text Editor!`n" -ForegroundColor Magenta

    $stream = $client.GetStream()
    $writer = New-Object System.IO.StreamWriter($stream)
    $writer.AutoFlush = $true

    function Send-Key([string]$key, [int]$delay = $KeyDelayMs) {
        $writer.WriteLine("sendkey $key")
        Start-Sleep -Milliseconds $delay
    }

    function Type-String([string]$str, [int]$delay = $KeyDelayMs) {
        foreach ($c in $str.ToCharArray()) {
            $k = switch ($c) {
                ' ' { 'spc' }
                "`n" { 'ret' }
                default { [string]$c }
            }
            Send-Key $k $delay
        }
    }

    # --- Live Demonstration Sequence ---

    Write-Host "[Typing Line 1] 'hello agentic os'..." -ForegroundColor Cyan
    Type-String "hello agentic os" $KeyDelayMs
    Start-Sleep -Milliseconds 600

    Write-Host "[Enter] Moving to row 1..." -ForegroundColor Cyan
    Send-Key "ret" 500

    Write-Host "[Typing Line 2] 'phase 13 text editor'..." -ForegroundColor Cyan
    Type-String "phase 13 text editor" $KeyDelayMs
    Start-Sleep -Milliseconds 600

    Write-Host "[Enter] Moving to row 2..." -ForegroundColor Cyan
    Send-Key "ret" 500

    Write-Host "[Typing Line 3] 'live ring 3 typing ok'..." -ForegroundColor Cyan
    Type-String "live ring 3 typing ok" $KeyDelayMs
    Start-Sleep -Milliseconds 800

    Write-Host "[2D Navigation] Moving cursor up and left..." -ForegroundColor Yellow
    Send-Key "up" 400
    Send-Key "up" 400
    Send-Key "left" 250
    Send-Key "left" 250
    Send-Key "left" 250

    Write-Host "[Editing] Backspacing and replacing letters..." -ForegroundColor Yellow
    Send-Key "backspace" 300
    Send-Key "backspace" 300
    Type-String "os" 300

    Write-Host "[Navigation] Returning cursor to end of text..." -ForegroundColor Yellow
    Send-Key "down" 300
    Send-Key "down" 300
    Send-Key "ret" 400
    Type-String "demo complete!" 200

    Write-Host "`n=== Live injection finished! ===" -ForegroundColor Green
    Write-Host "The Text Editor is still active in the window." -ForegroundColor White
    Write-Host "You can click into the window and continue typing yourself!" -ForegroundColor Cyan
    Write-Host "Press Ctrl + Alt + G to release mouse grab if captured.`n" -ForegroundColor Gray

    # Keep script open while window is active
    $writer.Close()
    $client.Close()
    $proc.WaitForExit()

} finally {
    if ($proc -and -not $proc.HasExited) {
        Write-Host "Stopping QEMU process $($proc.Id)..."
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}
