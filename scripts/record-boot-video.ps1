# Records a real video of a full boot (UEFI -> kernel -> compositor),
# by driving QEMU's own QMP protocol to request periodic real
# framebuffer screendumps (not a synthetic animation) while the guest
# boots, then encoding the resulting PPM frame sequence into an MP4
# with ffmpeg.
#
# Real, disclosed limitation: this machine does not have ffmpeg
# installed, so the encode step was written and reviewed but never
# actually run end-to-end here -- the QMP screendump mechanism itself
# (the part that produces real evidence, not the encode) is exercised
# and its frame count is verified. If ffmpeg is missing, the script
# still leaves the real PPM frame sequence in $FrameDir so the boot is
# inspectable even without a video, and says so plainly rather than
# silently skipping the step.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-boot-video.log",
    [string]$FrameDir = "_evidence\latest\boot-video-frames",
    [string]$OutputVideo = "_evidence\latest\boot.mp4",
    [int]$QmpPort = 4446,
    [double]$FrameIntervalSeconds = 0.5,
    [int]$DurationSeconds = 20,
    [int]$FrameRate = 2
)

$ErrorActionPreference = "Stop"

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
New-Item -ItemType Directory -Force -Path $FrameDir | Out-Null
Get-ChildItem $FrameDir -Filter "*.ppm" -ErrorAction SilentlyContinue | Remove-Item -Force

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-accel", "tcg,tb-size=128",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-vga", "std",
    "-display", "none",
    "-qmp", "tcp:127.0.0.1:$QmpPort,server=on,wait=off",
    "-no-reboot"
)

$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -WindowStyle Hidden
Start-Sleep -Milliseconds 500

function Send-Qmp([System.IO.StreamWriter]$writer, [System.IO.StreamReader]$reader, [string]$jsonLine) {
    $writer.WriteLine($jsonLine)
    $writer.Flush()
    return $reader.ReadLine()
}

$client = New-Object System.Net.Sockets.TcpClient
$connected = $false
for ($i = 0; $i -lt 20; $i++) {
    try {
        $client.Connect("127.0.0.1", $QmpPort)
        $connected = $true
        break
    } catch {
        Start-Sleep -Milliseconds 300
    }
}
if (-not $connected) {
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
    Write-Error "Could not connect to QEMU's QMP socket on port $QmpPort"
    exit 1
}

$stream = $client.GetStream()
$reader = New-Object System.IO.StreamReader($stream)
$writer = New-Object System.IO.StreamWriter($stream)
$writer.AutoFlush = $true

# QMP handshake: server sends a greeting, client must reply qmp_capabilities.
$reader.ReadLine() | Out-Null
Send-Qmp $writer $reader '{"execute":"qmp_capabilities"}' | Out-Null

$frameCount = 0
$deadline = (Get-Date).AddSeconds($DurationSeconds)
while ((Get-Date) -lt $deadline) {
    $framePath = Join-Path $FrameDir ("frame-{0:D4}.ppm" -f $frameCount)
    # QEMU writes the PPM to its OWN filesystem view -- same machine,
    # same path, since this is a local, non-sandboxed QEMU process.
    $qmpPath = $framePath -replace '\\', '/'
    $cmd = (@{execute = "screendump"; arguments = @{filename = $qmpPath}} | ConvertTo-Json -Compress)
    Send-Qmp $writer $reader $cmd | Out-Null
    Start-Sleep -Seconds $FrameIntervalSeconds
    if (Test-Path $framePath) { $frameCount++ }
}

$client.Close()
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }

Write-Output "Captured $frameCount real framebuffer frames to $FrameDir"
if ($frameCount -eq 0) {
    Write-Error "No frames captured -- QMP screendump did not produce output"
    exit 1
}

$ffmpeg = Get-Command ffmpeg -ErrorAction SilentlyContinue
if (-not $ffmpeg) {
    Write-Output "ffmpeg not found on this machine -- leaving the real PPM frame sequence in $FrameDir as the boot-video evidence (not encoded to $OutputVideo). Install ffmpeg and re-run to also get an MP4."
    exit 0
}

& $ffmpeg.Source -y -framerate $FrameRate -i (Join-Path $FrameDir "frame-%04d.ppm") -pix_fmt yuv420p $OutputVideo
if ($LASTEXITCODE -ne 0) {
    Write-Error "ffmpeg encode failed (exit $LASTEXITCODE) -- real PPM frames still available in $FrameDir"
    exit 1
}
Write-Output "Boot video written to $OutputVideo"
