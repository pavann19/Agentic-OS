# Phase 13 deliverable 1's own exit criterion (docs/ROADMAP.md):
# "Installing an app grants exactly the capabilities its manifest
# declared, nothing ambient -- an app that tries to use an undeclared
# capability is denied, demonstrated adversarially." Builds the kernel
# with the manifest_demo feature (off by default -- see
# kernel_rs/Cargo.toml), which mints a real Surface object (declared in
# the demo app's own manifest) and a real Socket object (deliberately
# NOT declared), asks thread::spawn_with_manifest to grant both, and
# verifies the Surface grant succeeds while the Socket grant is refused
# before it ever reaches the app's capability table -- real,
# adversarial denial, not a convention.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-manifest.log",
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

Write-Output "=== Building kernel WITH manifest_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "manifest_demo")
if ($exitCode -ne 0) { Write-Error "Kernel build (manifest_demo) failed"; exit 1 }
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
    @{ Name = "demo started, real Surface+Socket objects minted"; Pattern = "MANIFEST_DEMO_START" },
    @{ Name = "declared capability (Surface) granted and resolves"; Pattern = "MANIFEST_DEMO_DECLARED_GRANT_OK" },
    @{ Name = "undeclared capability (Socket) grant refused, real denial"; Pattern = "MANIFEST_DEMO_UNDECLARED_GRANT_DENIED_OK" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# Real negative checks -- these strings must NEVER appear.
$mustNotAppear = @("MANIFEST_DEMO_DECLARED_GRANT_UNEXPECTED_FAIL", "MANIFEST_DEMO_UNDECLARED_GRANT_LEAKED")
foreach ($bad in $mustNotAppear) {
    if ($content.Contains($bad)) {
        Write-Output "  FAIL: $bad appeared -- manifest enforcement is broken"
        $allPassed = $false
    } else {
        Write-Output "  PASS: no '$bad' -- enforcement held"
    }
}

if (-not $allPassed) {
    Write-Error "Manifest enforcement test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 13 deliverable 1 verified: an app was granted exactly the capability its manifest declared (a real Surface), while a second, undeclared capability (a real Socket) was refused at grant time before it ever reached the app's capability table -- demonstrated adversarially, not merely asserted."
