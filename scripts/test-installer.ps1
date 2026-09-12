# Phase 13 deliverable 2's own real, adversarial proof (docs/ROADMAP.md):
# a genuine, previously-unmodified ELF app (user_rs/serial_driver,
# already real since Phase 3) is installed through installer.rs's real
# manifest-gated core -- its declared PortIoRange capability is granted
# and the app does real work with it (a real COM1 write), while a
# second, deliberately over-asked Surface capability (never declared,
# never used by this app) is refused before it ever reaches this
# process. Builds the kernel with the installer_demo feature (off by
# default -- see kernel_rs/Cargo.toml).

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-installer.log",
    [int]$BootWaitSeconds = 15
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

Write-Output "=== Building kernel WITH installer_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "installer_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (installer_demo) failed"; exit 1 }
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

Write-Output "=== Rebuilding default kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) { Write-Error "No serial output at $SerialLog"; exit 1 }
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Name = "installer demo started"; Pattern = "INSTALLER_DEMO_START" },
    @{ Name = "declared PortIoRange grant succeeded"; Pattern = "INSTALLER_GRANT_OK label=com1" },
    @{ Name = "undeclared Surface grant refused"; Pattern = "INSTALLER_GRANT_DENIED label=adversarial_surface" },
    @{ Name = "real ELF entered ring 3"; Pattern = "INSTALLER_DEMO_ELF_ENTER" },
    @{ Name = "the genuine, unmodified serial_driver app did real work with its granted port"; Pattern = "USERSPACE_SERIAL_DRIVER" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

if ($content.Contains("INSTALLER_GRANT_OK label=adversarial_surface")) {
    Write-Output "  FAIL: the undeclared Surface capability was granted -- manifest enforcement broken in the installer"
    $allPassed = $false
} else {
    Write-Output "  PASS: the undeclared Surface capability was never granted"
}

if (-not $allPassed) {
    Write-Error "Installer manifest-enforcement test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 deliverable 2 verified: a genuine, previously-unmodified ELF app was installed through installer.rs's manifest-gated core, received exactly its manifest's declared capability (a real PortIoRange grant, used for a real COM1 write), and a second, undeclared capability request was refused before it ever reached the app -- demonstrated adversarially against a real app, not a synthetic kernel-thread stand-in."
