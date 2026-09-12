# Phase 12 (docs/ROADMAP.md Sec5, deliverable 1): minimal real
# compositor foundation. Builds the kernel WITH compositor_demo (off
# by default -- see kernel_rs/Cargo.toml), which makes
# compositor.rs/user_rs/compositor_driver own the real GOP framebuffer
# instead of user_driver.rs's own Phase 3 demo. Boots it and checks
# for the real two-Surface draw + independent kernel-side pixel
# readback: both real colors landed inside their own bounds, and the
# real gap between them (swept by a deliberate adversarial wide-fill
# attempt) stayed untouched -- real, falsifiable evidence Surface
# bounds are enforced in software, not just that two rectangles happen
# not to overlap. Rebuilds the default kernel afterward.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-compositor.log",
    [int]$BootWaitSeconds = 20
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

Write-Output "=== Building compositor_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\compositor_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "compositor_driver build failed"
    exit 1
}

Write-Output "=== Building kernel WITH compositor_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "compositor_demo")
if ($exitCode -ne 0) {
    Write-Error "Kernel build (compositor_demo) failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (Test-Path $SerialLog) { Clear-Content $SerialLog }
$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
Start-Sleep -Seconds $BootWaitSeconds
if (-not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

Write-Output "=== Rebuilding default (non-feature) kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    "COMPOSITOR_FB_MAPPED",
    "COMPOSITOR_SURFACES_GRANTED",
    "COMPOSITOR_SELF_CHECK_DRAWN",
    "COMPOSITOR_READY",
    "COMPOSITOR_READBACK",
    "COMPOSITOR_SELF_CHECK_PASS"
)
foreach ($c in $checks) {
    if ($content.Contains($c)) {
        Write-Output "  PASS: $c"
    } else {
        Write-Output "  FAIL: pattern not found: $c"
        $allPassed = $false
    }
}

if ($content.Contains("COMPOSITOR_SELF_CHECK_FAIL")) {
    Write-Output "  FAIL: COMPOSITOR_SELF_CHECK_FAIL appeared -- readback did not match expected colors/bounds"
    $allPassed = $false
}

if (-not $allPassed) {
    Write-Error "Compositor self-check FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Compositor foundation verified: two real Surface capabilities minted and granted, a real ELF-loaded process drew a distinct real color into each within its own bounds, and an independent kernel-side pixel readback confirmed both colors landed correctly AND that the real gap between them -- swept by a deliberate adversarial wide-fill attempt -- was left untouched."
