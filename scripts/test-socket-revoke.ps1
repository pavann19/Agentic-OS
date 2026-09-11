# Phase 10 exit criterion 3 (docs/ROADMAP.md): "Revoking a process's
# socket capability mid-connection terminates its access immediately —
# demonstrated adversarially, same bar Phase 2's capability revocation
# was held to." Builds the kernel WITH socket_revoke_demo (off by
# default -- see kernel_rs/Cargo.toml), boots it, and checks the real
# use -> revoke -> refuse chain from kernel_rs/src/socket_demo.rs.
# Rebuilds the default kernel afterward, same discipline as
# scripts/test-supervisor.ps1.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [string]$FatDir = "boot_rs\qemu_fatdir",
    [string]$SerialLog = "_evidence\latest\serial-socket-revoke.log",
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

Write-Output "=== Building kernel WITH socket_revoke_demo ==="
$exitCode = Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none", "--features", "socket_revoke_demo")
if ($exitCode -ne 0) {
    Write-Error "socket_revoke_demo kernel build failed"
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

Write-Output "=== Rebuilding default kernel ==="
Invoke-CargoQuiet "kernel_rs" @("build", "--release", "--target", "x86_64-unknown-none") | Out-Null
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial output at $SerialLog"
    exit 1
}
$content = Get-Content $SerialLog -Raw

$allPassed = $true
$checks = @(
    @{ Pattern = "SOCKET_DEMO_START"; Name = "real Socket capability object created" },
    @{ Pattern = "SOCKET_DEMO_USE_OK"; Name = "holder thread resolved the Socket capability BEFORE revoke" },
    @{ Pattern = "SOCKET_DEMO_REVOKE_ISSUED"; Name = "revoker thread issued a real capability::revoke, mid-use" },
    @{ Pattern = "SOCKET_DEMO_REVOKED_OK"; Name = "holder thread's SAME capability now refuses to resolve after revoke" }
)
foreach ($c in $checks) {
    if ($content -match [regex]::Escape($c.Pattern)) {
        Write-Output "  PASS: $($c.Name)"
    } else {
        Write-Output "  FAIL: $($c.Name) -- pattern not found: $($c.Pattern)"
        $allPassed = $false
    }
}

# Real, necessary negative checks: neither failure-mode log line may
# appear, or the "pass" lines above would be misleading about what
# actually happened.
foreach ($bad in @("SOCKET_DEMO_USE_FAIL_UNEXPECTED", "SOCKET_DEMO_REVOKE_FAILED", "SOCKET_DEMO_TIMEOUT", "SOCKET_DEMO_REVOKER_TIMEOUT")) {
    if ($content -match [regex]::Escape($bad)) {
        Write-Output "  FAIL: $bad appeared -- see $SerialLog"
        $allPassed = $false
    }
}

if (-not $allPassed) {
    Write-Error "Socket capability revocation test FAILED -- see log at $SerialLog"
    exit 1
}

Write-Output ""
Write-Output "Phase 10 exit criterion 3 verified: a real Socket capability was granted, resolved successfully by its holder, then revoked mid-use by a second real thread, and the SAME capability immediately refused to resolve again -- the identical generic capability::revoke mechanism Phase 2's own revocation demo proved, now exercised against a real Socket object."
