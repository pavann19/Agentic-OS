# Launch QEMU interactively for manual inspection with WHPX acceleration

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot

$repoTemp = Join-Path $repoRoot "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

$qemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe"
$ovmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd"
$fatDir = "boot_rs\qemu_fatdir"
$logFile = Join-Path $repoRoot "_evidence\latest\serial-manual.log"
New-Item -ItemType Directory -Force -Path (Split-Path $logFile) | Out-Null
if (Test-Path $logFile) { Remove-Item $logFile -Force }

$qemuArgs = @(
    "-machine", "q35,accel=whpx,kernel-irqchip=on",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$ovmfCode`"",
    "-drive", "file=fat:rw:$fatDir,format=raw",
    "-serial", "file:$logFile",
    "-name", "Agentic-OS"
)

$proc = Start-Process -FilePath $qemuExe -ArgumentList $qemuArgs -PassThru -RedirectStandardError "_evidence\latest\qemu-stderr.log" -RedirectStandardOutput "_evidence\latest\qemu-stdout.log"
Start-Sleep -Milliseconds 2000
if ($proc.HasExited) {
    if (Test-Path "_evidence\latest\qemu-stderr.log") {
        Get-Content "_evidence\latest\qemu-stderr.log"
    }
    Write-Error "QEMU exited with code $($proc.ExitCode)"
} else {
    Write-Output "QEMU running successfully with PID $($proc.Id)"
}
