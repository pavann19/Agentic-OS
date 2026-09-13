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

Write-Output "=== Building window_client_driver ==="
$exitCode = Invoke-CargoQuiet "user_rs\window_client_driver" @("build", "--release", "--target", "x86_64-unknown-none")
if ($exitCode -ne 0) {
    Write-Error "window_client_driver build failed"
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
    "COMPOSITOR_SELF_CHECK_PASS",
    "WINDOW_CLIENT_SURFACE_GRANTED label=A",
    "WINDOW_CLIENT_SURFACE_GRANTED label=B",
    "SURFACE_FILL_OK",
    "FOREIGN_CAP_DENIED_OK",
    "COMPOSITOR_MULTIPROC_READBACK",
    "COMPOSITOR_MULTIPROC_SELF_CHECK_PASS",
    "TEXT_FONT_INIT",
    "TEXT_DRAWN",
    "COMPOSITOR_TEXT_READBACK",
    "COMPOSITOR_TEXT_SELF_CHECK_PASS",
    "BITMAP_DRAWN",
    "COMPOSITOR_BITMAP_READBACK",
    "COMPOSITOR_BITMAP_SELF_CHECK_PASS"
)
foreach ($c in $checks) {
    if ($content.Contains($c)) {
        Write-Output "  PASS: $c"
    } else {
        Write-Output "  FAIL: pattern not found: $c"
        $allPassed = $false
    }
}

foreach ($bad in @("COMPOSITOR_SELF_CHECK_FAIL", "COMPOSITOR_MULTIPROC_SELF_CHECK_FAIL", "SURFACE_FILL_UNEXPECTED_DENIAL", "FOREIGN_CAP_UNEXPECTEDLY_SUCCEEDED", "COMPOSITOR_BITMAP_SELF_CHECK_FAIL")) {
    if ($content.Contains($bad)) {
        Write-Output "  FAIL: $bad appeared -- see $SerialLog"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Compositor self-check FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Compositor foundation verified, including Phase 12 exit criterion 1: two real Surface capabilities minted and granted to a single process, drawn correctly with bounds enforced against a deliberate adversarial wide-fill attempt; AND two SEPARATE real processes, each in its own address space with its own capability table and no framebuffer MMIO access at all, each drew only its own real Surface through a kernel-mediated syscall, with a foreign-CapId adversarial probe correctly denied by both -- independently confirmed by a kernel-side pixel readback."
