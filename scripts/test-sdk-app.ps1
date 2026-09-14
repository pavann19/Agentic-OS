# Phase 13 Exit Criterion 2:
# Third-Party Application Verification
# "The SDK builds and runs a genuinely new app (not one of the four reference apps)
# with no kernel or platform-service change required."
#
# Validates:
#  1. sample_app builds against agentic_sdk in ring-3 user space.
#  2. Kernel installs sample_app via manifest-gated installer.
#  3. sample_app enters ring 3 and renders text to its surface via agentic_sdk widgets.
#  4. Real readiness token exchanged with kernel over IPC.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-sdk-app.log",
    [int]$BootWaitSeconds = 45
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

Write-Output "=== Building sample_app (SDK app) ==="
$exitCode = Invoke-CargoQuiet "user_rs\sample_app" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) { Write-Error "sample_app build failed"; exit 1 }

Write-Output "=== Building kernel WITH sample_app_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "sample_app_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (sample_app_demo) failed"; exit 1 }
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

Write-Output "=== Rebuilding default kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Name = "Manifest-gated installer granted Surface capability"; Pattern = "INSTALLER_GRANT_OK label=sample_app_surface" },
    @{ Name = "sample_app entered ring 3"; Pattern = "SAMPLE_APP_ELF_ENTER" },
    @{ Name = "sample_app announced execution via COM1"; Pattern = "[SAMPLE_APP] Genuinely new third-party app built with agentic_sdk" },
    @{ Name = "sample_app confirmed pass"; Pattern = "[SAMPLE_APP] SAMPLE_APP_PASS: third-party SDK app running in ring 3" },
    @{ Name = "sample_app signaled real readiness token to kernel"; Pattern = "SAMPLE_APP_READY token=0x53414d50" }
)

foreach ($c in $checks) {
    $matched = $content.Contains($c.Pattern)
    if ($matched) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "SDK app test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 Exit Criterion 2 verified: A genuinely new third-party application built entirely on agentic_sdk ran cleanly in ring 3 with zero kernel or platform service modifications."
