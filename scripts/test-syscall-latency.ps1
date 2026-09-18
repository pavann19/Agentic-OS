# Phase 8, deliverable 5's "one measured systems metric" -- the piece of
# the original CI/evidence plan that landed everywhere else (CI, boot,
# revocation, fuzzing, docs/DECISIONS.md, the demo video) except here.
# Boots the DEFAULT kernel image (no feature flags -- this metric is
# produced by user_rs/serial_driver, the earliest real ring-3 ELF spawned
# on every boot) and asserts a real [SYSCALL_LATENCY] marker with sane
# values, not just presence -- see user_rs/serial_driver/src/main.rs for
# exactly what's measured and why (syscall 1, not SYS_YIELD -- that
# choice, and the two toolchain bugs found getting a stack array to
# survive at all, are documented in that file, not repeated here).
#
# Honest scope, same as docs/PERFORMANCE_BASELINE.md: these are Tier 1
# (QEMU/TCG software emulation) numbers, not a hardware measurement.
# TCG's per-instruction interpretation overhead means the microsecond
# figures here are expected to be one to three orders of magnitude
# slower than real silicon -- the value of this test is REGRESSION
# tracking (did a change make syscall dispatch meaningfully slower/more
# variable on this same emulated platform), not an absolute latency
# claim about eventual real hardware.

param(
    [string]$QemuExe = $(if ($env:AGENTIC_OS_QEMU) { $env:AGENTIC_OS_QEMU }
        elseif (Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue) { (Get-Command qemu-system-x86_64).Source }
        else { "C:\Program Files\qemu\qemu-system-x86_64.exe" }),
    [string]$OvmfCode = $(if ($env:AGENTIC_OS_OVMF_CODE) { $env:AGENTIC_OS_OVMF_CODE }
        elseif (Test-Path "/usr/share/OVMF/OVMF_CODE.fd") { "/usr/share/OVMF/OVMF_CODE.fd" }
        else { "C:\Program Files\qemu\share\edk2-x86_64-code.fd" }),
    [string]$FatDir = "boot_rs/qemu_fatdir",
    [string]$SerialLog = "_evidence/latest/serial-syscall-latency.log",
    [string]$DiskImage = "_evidence/disk-syscall-latency.img",
    [int]$TimeoutSeconds = 20
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$repoTemp = Join-Path (Get-Location) "_evidence\qemu_tmp"
New-Item -ItemType Directory -Force -Path $repoTemp | Out-Null
$env:TMP = $repoTemp
$env:TEMP = $repoTemp

New-Item -ItemType Directory -Force -Path (Split-Path $SerialLog) | Out-Null
Remove-Item -Force $SerialLog -ErrorAction SilentlyContinue

New-Item -ItemType Directory -Force -Path (Split-Path $DiskImage) | Out-Null
if (-not (Test-Path $DiskImage)) {
    $fs = [System.IO.File]::Create($DiskImage)
    $fs.SetLength(16MB)
    $fs.Close()
}

Write-Output "=== Building default kernel (no feature flags) ==="
# `cargo`'s own `.cargo/config.toml` resolution walks up from the
# CURRENT DIRECTORY, not from `--manifest-path` -- kernel_rs's target/
# rustflags/build-std settings live in kernel_rs/.cargo/config.toml, so
# building from the repo root via `--manifest-path` silently misses them
# (a real failure hit writing this script: "cannot find entry symbol
# _start", the exact symptom of losing the freestanding target config).
# Push-Location into kernel_rs first, matching every other test script's
# own `Invoke-CargoQuiet` helper.
Push-Location kernel_rs
try {
    & cargo build --release --target x86_64-unknown-none
    $buildExitCode = $LASTEXITCODE
} finally {
    Pop-Location
}
if ($buildExitCode -ne 0) {
    Write-Error "Kernel build failed"
    exit 1
}
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$FatDir\kernel.elf" -Force

$qemuArgs = @(
    "-machine", "q35,kernel-irqchip=split",
    "-m", "256M",
    "-device", "intel-iommu,intremap=on",
    "-drive", "if=pflash,format=raw,readonly=on,file=`"$OvmfCode`"",
    "-drive", "file=fat:rw:$FatDir,format=raw",
    "-device", "virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on,ats=on",
    "-drive", "file=$DiskImage,if=none,id=disk0,format=raw",
    "-serial", "file:$SerialLog",
    "-display", "none",
    "-no-reboot"
)

Write-Output "=== Booting and waiting for SYSCALL_LATENCY marker ==="
$proc = Start-Process -FilePath $QemuExe -ArgumentList $qemuArgs -PassThru -NoNewWindow
$exited = $proc.WaitForExit($TimeoutSeconds * 1000)
if (-not $exited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
}

if (-not (Test-Path $SerialLog)) {
    Write-Error "No serial log produced at $SerialLog"
    exit 1
}

$content = Get-Content $SerialLog -Raw

$required = @("BOOT_START", "EXIT_BOOT_SERVICES_OK", "KERNEL_ENTER", "REVOCATION_REJECTED_OK")
$missing = $required | Where-Object { $content -notmatch [regex]::Escape($_) }
if ($missing.Count -gt 0) {
    Write-Error "Missing boot checkpoint(s): $($missing -join ', ') in $SerialLog"
    exit 1
}
Write-Output "PASS: boot checkpoints confirmed"

$match = [regex]::Match($content, "\[SYSCALL_LATENCY\] samples=(\d+) median_cycles=(\d+) min_cycles=(\d+) max_cycles=(\d+) median_ns_est=(\d+)")
if (-not $match.Success) {
    Write-Error "No [SYSCALL_LATENCY] marker found in $SerialLog"
    Write-Output "--- serial log ---"
    Write-Output $content
    exit 1
}

$samples = [int64]$match.Groups[1].Value
$medianCycles = [int64]$match.Groups[2].Value
$minCycles = [int64]$match.Groups[3].Value
$maxCycles = [int64]$match.Groups[4].Value
$medianNsEst = [int64]$match.Groups[5].Value

# Real assertions, not just presence: 63 real samples were taken, the
# ordering min <= median <= max must hold (it's the same sorted array),
# and a non-zero syscall genuinely costs at least a few hundred cycles
# on real hardware, let alone under TCG interpretation -- a median at or
# near zero would mean the timing loop itself is broken (e.g. measuring
# nothing), not that syscalls got impossibly fast.
if ($samples -ne 63) {
    Write-Error "Expected samples=63, got $samples"
    exit 1
}
if (-not ($minCycles -le $medianCycles -and $medianCycles -le $maxCycles)) {
    Write-Error "min/median/max out of order: min=$minCycles median=$medianCycles max=$maxCycles"
    exit 1
}
if ($medianCycles -lt 100) {
    Write-Error "median_cycles=$medianCycles is implausibly low for a real syscall round trip -- timing loop likely broken"
    exit 1
}
Write-Output "PASS: real [SYSCALL_LATENCY] marker, samples=$samples median_cycles=$medianCycles min_cycles=$minCycles max_cycles=$maxCycles"

$result = [ordered]@{
    measured_at_utc = (Get-Date).ToUniversalTime().ToString("o")
    platform        = "Tier 1 (QEMU q35 + OVMF + TCG software emulation) -- NOT real hardware, see docs/SUPPORTED_HARDWARE.md"
    what            = "syscall 1 (SYSCALL_LOG) round trip from ring 3, measured with RDTSC, user_rs/serial_driver/src/main.rs"
    samples         = $samples
    median_cycles   = $medianCycles
    min_cycles      = $minCycles
    max_cycles      = $maxCycles
    median_ns_est   = $medianNsEst
    ns_est_note     = "nominal ~2.5GHz estimate (kernel_rs::compositor_metrics::CYCLES_PER_US), not calibrated -- cycle counts above are the real measured numbers"
}
$result | ConvertTo-Json -Depth 5 | Set-Content "_evidence\syscall-latency-result.json"
Write-Output ""
Write-Output "Raw result: _evidence\syscall-latency-result.json"

exit 0
