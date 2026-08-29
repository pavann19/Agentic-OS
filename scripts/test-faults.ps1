# Fault-injection test suite (docs/ROADMAP.md Phase 0 item). Builds four
# kernel variants, each deliberately triggering one fault, and asserts each
# produces the correct diagnosable exception report on serial rather than a
# silent hang. See kernel_rs/src/fault_injection.rs.

param(
    [string]$QemuExe = "C:\Program Files\qemu\qemu-system-x86_64.exe",
    [string]$OvmfCode = "C:\Program Files\qemu\share\edk2-x86_64-code.fd",
    [int]$TimeoutSeconds = 15
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

# cargo's normal "Compiling..." status lines go to stderr by design, and
# this PowerShell (5.1) treats a native command's stderr output as a
# terminating error under $ErrorActionPreference="Stop" regardless of
# stream redirection (2>&1 / 2>$null / *>$null all still trigger it) --
# confirmed by testing, not assumed. Cargo calls below wrap
# $ErrorActionPreference to "Continue" locally and restore it after, since
# that's the one thing that actually avoids the false-positive abort here.
function Invoke-CargoQuiet {
    param([string[]]$CargoArgs)
    $saved = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo @CargoArgs 2>&1 | Out-Null
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $saved
    }
}

# cargo isn't on PATH in the environment `make` hands this script -- and
# $env:USERPROFILE itself is unreliable here too (make/MSYS2 strips more
# than just TMP/TEMP from what it passes to child processes, confirmed
# repeatedly in this session). Hardcoded path, matching every other place
# in this repo that had to work around the same issue.
$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

$repoTemp = Join-Path $repoRoot "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

$cases = @(
    @{ Feature = "fault_test_null_deref"; ExpectVector = "vector=14"; Name = "null dereference (page fault)" },
    @{ Feature = "fault_test_rodata_write"; ExpectVector = "vector=14"; Name = "write to read-only .rodata (page fault)" },
    @{ Feature = "fault_test_nx_exec"; ExpectVector = "vector=14"; Name = "execute NX .data (page fault)" },
    @{ Feature = "fault_test_double_fault"; ExpectVector = "vector=8"; Name = "stack overflow (double fault, IST)" }
)

$fatDir = "boot_rs\qemu_fatdir"
$allPassed = $true

foreach ($case in $cases) {
    Write-Output "=== $($case.Name) [$($case.Feature)] ==="

    Push-Location kernel_rs
    $exitCode = Invoke-CargoQuiet @("build", "--release", "--features", $case.Feature)
    $buildOk = $exitCode -eq 0
    Pop-Location

    if (-not $buildOk) {
        Write-Output "  BUILD FAILED"
        $allPassed = $false
        continue
    }

    Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$fatDir\kernel.elf" -Force

    $logPath = "_evidence\latest\fault_$($case.Feature).log"
    $qemuArgs = @(
        "-machine", "q35",
        "-m", "256M",
        "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
        "-drive", "file=fat:rw:$fatDir,format=raw",
        "-serial", "file:$logPath",
        "-display", "none",
        "-no-reboot"
    )
    $proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
    $exited = $proc.WaitForExit($TimeoutSeconds * 1000)
    if (-not $exited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }

    if (-not (Test-Path $logPath)) {
        Write-Output "  FAIL: no serial log produced"
        $allPassed = $false
        continue
    }
    $content = Get-Content $logPath -Raw
    if ($content -match [regex]::Escape($case.ExpectVector)) {
        Write-Output "  PASS: found $($case.ExpectVector)"
    } else {
        Write-Output "  FAIL: expected $($case.ExpectVector), not found in $logPath"
        $allPassed = $false
    }
}

# Rebuild the normal (no fault-test feature) kernel so the tree is left in
# its default state, not mid-fault-injection-variant.
Push-Location kernel_rs
Invoke-CargoQuiet @("build", "--release") | Out-Null
Pop-Location
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$fatDir\kernel.elf" -Force

if ($allPassed) {
    Write-Output "All fault-injection cases passed."
    exit 0
} else {
    Write-Output "One or more fault-injection cases FAILED."
    exit 1
}
