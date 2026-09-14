# Phase 12 & Phase 10 Master Test Orchestrator
# Executes all test suites for Phase 12 and Phase 10 deliverables:
# 1. VirtIO-GPU 2D Hardware Acceleration (test-virtio-gpu.ps1)
# 2. Typed Agent UI Introspection & Direct Action (test-agent-ui.ps1)
# 3. Multi-NIC Routing, Loopback & DNS Capabilities (test-multi-nic.ps1)
# 4. Compositor Foundation Zero-Regression (test-compositor.ps1)
# 5. Baseline Kernel Boot Zero-Regression (test-boot.ps1)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
[Environment]::CurrentDirectory = $repoRoot

$suites = @(
    "scripts\test-virtio-gpu.ps1",
    "scripts\test-agent-ui.ps1",
    "scripts\test-multi-nic.ps1",
    "scripts\test-compositor.ps1",
    "scripts\test-boot.ps1"
)

$passed = 0
$total = $suites.Length

foreach ($s in $suites) {
    Write-Output "================================================================="
    Write-Output "Running Test Suite: $s"
    Write-Output "================================================================="
    
    $proc = Start-Process -FilePath "powershell.exe" -ArgumentList @("-ExecutionPolicy", "Bypass", "-File", $s) -PassThru -NoNewWindow -Wait
    if ($proc.ExitCode -eq 0) {
        Write-Output ">>> SUITE PASSED: $s"
        $passed += 1
    } else {
        Write-Error ">>> SUITE FAILED (exit code $($proc.ExitCode)): $s"
        exit 1
    }
}

Write-Output ""
Write-Output "================================================================="
Write-Output "MASTER VERIFICATION COMPLETE: $passed / $total SUITES PASSED (100%)"
Write-Output "Phase 12 & Phase 10 Deliverables Fully Verified with ZERO Regressions"
Write-Output "================================================================="
exit 0
