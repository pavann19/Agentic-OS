# Phase 5.0: Compositor Performance Telemetry & Baseline Benchmarking
#
# Runs the 6 empirical baseline scenarios against the running OS:
#  1. Idle Desktop (5s)
#  2. Mouse Movement Across Desktop
#  3. Window Dragging Across Desktop
#  4. Fast Typing into Active Window
#  5. Multi-Window Concurrent Execution (4 apps: Terminal, Text Editor, File Manager, Net Client)
#  6. Rapid Mouse Movement
#
# Parses cycle-accurate TSC telemetry dumped via [GUI_BASELINE_SUMMARY]
# and emits formatted console table + _evidence/baseline_metrics.json.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-benchmark-baseline.log",
    [string]$DiskImg = "_evidence\latest\disk-benchmark-baseline.img",
    [string]$JsonOut = "_evidence\baseline_metrics.json",
    [int]$MonitorPort = 45470,
    [int]$MaxBootWaitSeconds = 60
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

Write-Output "============================================================"
Write-Output "  Phase 5.0: Performance Baseline & Telemetry Benchmark     "
Write-Output "============================================================"

Write-Output "=== 1. Building Drivers & Reference Apps ==="
$apps = @(
    "user_rs\terminal_emulator",
    "user_rs\text_editor",
    "user_rs\virtio_blk_driver",
    "user_rs\file_manager",
    "user_rs\netstack_driver",
    "user_rs\net_client",
    "user_rs\mouse_driver"
)
foreach ($app in $apps) {
    Write-Output "  Building $app..."
    $exitCode = Invoke-CargoQuiet $app @("build", "--release", "--target", "x86_64-unknown-none")
    if ($exitCode -ne 0) { Write-Error "$app build failed"; exit 1 }
}

Write-Output "=== 2. Building Kernel WITH phase13_all (Full Multi-Window Desktop) ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "phase13_all")
if ($exitCode -ne 0) { Write-Error "Kernel build failed"; exit 1 }
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
    "-monitor", "tcp:127.0.0.1:$MonitorPort,server,nowait",
    "-display", "none",
    "-no-reboot"
)

Write-Output "=== 3. Launching QEMU Benchmark Target ==="
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
try {
    Write-Output "  Waiting for system boot and desktop launch..."
    $booted = $false
    for ($w = 0; $w -lt $MaxBootWaitSeconds; $w++) {
        Start-Sleep -Seconds 1
        if (Test-Path $SerialLog) {
            $c = Get-Content $SerialLog -Raw -ErrorAction SilentlyContinue
            if ($c -and $c.Contains("LAUNCHER_ALL_APPS_READY_PASS")) {
                $booted = $true
                Write-Output "  System booted and all 4 desktop apps ready after $w seconds."
                break
            }
        }
    }
    if (-not $booted) {
        Write-Output "  Note: Proceeding after reaching wait limit..."
    }

    Start-Sleep -Seconds 2

    $client = New-Object System.Net.Sockets.TcpClient
    $client.Connect("127.0.0.1", $MonitorPort)
    $stream = $client.GetStream()
    $writer = New-Object System.IO.StreamWriter($stream)
    $writer.AutoFlush = $true

    function Send-Key-Marker {
        param([string]$Key, [string]$MarkerPattern)
        for ($retry = 0; $retry -lt 3; $retry++) {
            $writer.WriteLine("sendkey $Key")
            Start-Sleep -Milliseconds 600
            if (Test-Path $SerialLog) {
                $content = Get-Content $SerialLog -Raw -ErrorAction SilentlyContinue
                if ($content -and $content.Contains($MarkerPattern)) {
                    return
                }
            }
            Start-Sleep -Milliseconds 400
        }
    }

    Write-Output "=== 4. Executing Benchmark Scenarios ==="

    # Reset metrics & start Scenario 1
    Write-Output "  Starting Scenario 1: Idle Desktop (5s)..."
    Send-Key-Marker "f1" "scenario=idle"
    Start-Sleep -Seconds 5

    # End Scenario 1 -> Start Scenario 2
    Write-Output "  Starting Scenario 2: Mouse Movement Across Desktop..."
    Send-Key-Marker "f2" "scenario=mouse_motion"
    Start-Sleep -Milliseconds 500
    for ($i = 0; $i -lt 15; $i++) {
        $writer.WriteLine("mouse_move 20 15")
        Start-Sleep -Milliseconds 200
    }

    # End Scenario 2 -> Start Scenario 3
    Write-Output "  Starting Scenario 3: Window Dragging Across Desktop..."
    Send-Key-Marker "f3" "scenario=window_drag"
    Start-Sleep -Milliseconds 500
    # Click and drag titlebar
    $writer.WriteLine("mouse_button 1")
    Start-Sleep -Milliseconds 300
    for ($i = 0; $i -lt 10; $i++) {
        $writer.WriteLine("mouse_move 15 10")
        Start-Sleep -Milliseconds 200
    }
    $writer.WriteLine("mouse_button 0")
    Start-Sleep -Milliseconds 500

    # End Scenario 3 -> Start Scenario 4
    Write-Output "  Starting Scenario 4: Fast Typing into Active Window..."
    Send-Key-Marker "f4" "scenario=typing"
    Start-Sleep -Milliseconds 500
    foreach ($k in @("a", "b", "c", "d", "e", "f", "g", "ret", "1", "2", "3", "4", "ret")) {
        $writer.WriteLine("sendkey $k")
        Start-Sleep -Milliseconds 150
    }
    Start-Sleep -Seconds 1

    # End Scenario 4 -> Start Scenario 5
    Write-Output "  Starting Scenario 5: Multi-Window Concurrent Switching & Typing..."
    Send-Key-Marker "f5" "scenario=multi_window"
    Start-Sleep -Milliseconds 500
    # Move to top-right window (Text Editor) and click
    $writer.WriteLine("mouse_move 200 -50")
    Start-Sleep -Milliseconds 200
    $writer.WriteLine("mouse_button 1")
    Start-Sleep -Milliseconds 100
    $writer.WriteLine("mouse_button 0")
    Start-Sleep -Milliseconds 300
    foreach ($k in @("t", "e", "s", "t", "ret")) {
        $writer.WriteLine("sendkey $k")
        Start-Sleep -Milliseconds 150
    }
    Start-Sleep -Milliseconds 500
    # Move to bottom-left window (File Manager) and click
    $writer.WriteLine("mouse_move -200 200")
    Start-Sleep -Milliseconds 200
    $writer.WriteLine("mouse_button 1")
    Start-Sleep -Milliseconds 100
    $writer.WriteLine("mouse_button 0")
    Start-Sleep -Milliseconds 500

    # End Scenario 5 -> Start Scenario 6
    Write-Output "  Starting Scenario 6: Rapid Mouse Movement..."
    Send-Key-Marker "f6" "scenario=rapid_mouse"
    Start-Sleep -Milliseconds 500
    for ($i = 0; $i -lt 20; $i++) {
        $dx = if ($i % 2 -eq 0) { 35 } else { -30 }
        $dy = if ($i % 3 -eq 0) { 25 } else { -20 }
        $writer.WriteLine("mouse_move $dx $dy")
        Start-Sleep -Milliseconds 100
    }

    # End Scenario 6 and dump final summary
    Write-Output "  Concluding benchmarks..."
    Send-Key-Marker "f7" "BENCHMARK_SCENARIO_END"
    Start-Sleep -Seconds 2

    $writer.Close()
    $client.Close()
    Write-Output "=== 5. Scenarios Completed ==="
} finally {
    if (-not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
}

Write-Output "=== 6. Rebuilding Default Kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial log produced at $SerialLog"
    exit 1
}

$logContent = Get-Content $SerialLog
$summaryLines = $logContent | Where-Object { $_ -match "\[GUI_BASELINE_SUMMARY\]" }

if ($summaryLines.Count -eq 0) {
    Write-Error "No [GUI_BASELINE_SUMMARY] telemetry lines found in $SerialLog!"
    exit 1
}

Write-Output ""
Write-Output "=========================================================================================================="
Write-Output "                                 PHASE 5.0 COMPOSITOR BASELINE RESULTS                                   "
Write-Output "=========================================================================================================="

$seenScenarios = [ordered]@{}

foreach ($line in $summaryLines) {
    $entry = [ordered]@{}
    $parts = $line.Substring($line.IndexOf("[GUI_BASELINE_SUMMARY]") + 23).Trim().Split(" ")
    foreach ($p in $parts) {
        if ($p -match "^([^=]+)=(.+)$") {
            $key = $Matches[1]
            $val = $Matches[2]
            if ($val -match "^\d+$") {
                $entry[$key] = [int64]$val
            } else {
                $entry[$key] = $val
            }
        }
    }
    $scen = $entry["scenario"]
    if (-not $seenScenarios.Contains($scen) -or ($entry["sample_count"] -gt 0 -and $seenScenarios[$scen].sample_count -eq 0)) {
        $seenScenarios[$scen] = [PSCustomObject]$entry
    }
}

$results = @($seenScenarios.Values)

# Print formatted table
$results | Format-Table -Property scenario, total_frames, sample_count, avg_frame_us, avg_compose_us, avg_flush_us, avg_cursor_us, avg_input_us, avg_damage_us, avg_damaged_rects, avg_damaged_scanlines, avg_damaged_pixels, mouse_events, key_events -AutoSize

# Save results to JSON artifact
New-Item -ItemType Directory -Force -Path (Split-Path $JsonOut) | Out-Null
$jsonString = $results | ConvertTo-Json -Depth 4
Set-Content -Path $JsonOut -Value $jsonString -Force

Write-Output "Baseline metrics saved to: $JsonOut"
Write-Output "Phase 5.0 baseline instrumentation and capture completed successfully!"
