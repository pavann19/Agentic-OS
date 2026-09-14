# Phase 5.15: Compositor Performance Benchmark
# Measures composition pass time, backbuffer blit time, occlusion clipping, and frame presentation.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-benchmark-compositor.log",
    [int]$WaitSeconds = 25
)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
[Environment]::CurrentDirectory = $repoRoot

$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }

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

Write-Output "=== Benchmark: Compositor Rendering & Occlusion Pipeline ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "compositor_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build failed"; exit 1 }
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
if (Test-Path $SerialLog) { Remove-Item $SerialLog -Force }

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $WaitSeconds
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }

# Rebuild default kernel
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) { Write-Error "No output log generated"; exit 1 }
$log = Get-Content $SerialLog -Raw

Write-Output "--- Compositor Performance Summary ---"
$frames = 0
$totalUs = 0
$composeUs = 0
$flushUs = 0

foreach ($line in ($log -split "`n")) {
    if ($line -match "COMPOSITOR_METRICS.*frame=(\d+)us.*compose=(\d+)us.*flush=(\d+)us") {
        $frames++
        $totalUs += [int64]$Matches[1]
        $composeUs += [int64]$Matches[2]
        $flushUs += [int64]$Matches[3]
    }
}

$avgFrame = if ($frames -gt 0) { [math]::Round($totalUs / $frames, 2) } else { 0 }
$avgCompose = if ($frames -gt 0) { [math]::Round($composeUs / $frames, 2) } else { 0 }
$avgFlush = if ($frames -gt 0) { [math]::Round($flushUs / $frames, 2) } else { 0 }

[PSCustomObject]@{
    Benchmark = "Compositor"
    TotalFrames = $frames
    AvgFrameTimeUs = $avgFrame
    AvgComposeTimeUs = $avgCompose
    AvgFlushTimeUs = $avgFlush
    Renderer = "CpuRenderer (Double-Buffered GOP Scanlines)"
} | Format-Table -AutoSize
