# Phase 8, deliverable 3: the full automated suite in one pass, "from a
# clean checkout with no manual steps" (the phase's own exit
# criterion). Runs every real test script this project has, in
# dependency order (cheap/host-only first, so a fast failure doesn't
# wait on a dozen QEMU boots first), aggregates a real pass/fail
# report, and exits non-zero if anything failed -- a single command a
# CI system (or a human) can run and trust.
#
# Scope, stated honestly: this covers everything verifiable on Tier 1
# (QEMU). It does NOT include a Tier 2 physical-hardware pass --
# that's `docs/PROGRESS.md`'s own disclosed, non-code-closable gap
# (see the Phase 3/8 sections there), not something this script
# glosses over as covered.

$ErrorActionPreference = "Continue"

$suite = @(
    @{ Name = "test-host";                    Script = "scripts\test-host.ps1" },
    @{ Name = "test-boot";                     Script = "scripts\test-boot.ps1" },
    @{ Name = "test-faults";                   Script = "scripts\test-faults.ps1" },
    @{ Name = "test-keyboard";                 Script = "scripts\test-keyboard.ps1" },
    @{ Name = "test-powerloss";                Script = "scripts\test-powerloss.ps1" },
    @{ Name = "test-synthesis";                Script = "scripts\test-synthesis.ps1" },
    @{ Name = "test-synthesis-fault-demo";     Script = "scripts\test-synthesis-fault-demo.ps1" },
    @{ Name = "test-ahci";                     Script = "scripts\test-ahci.ps1" },
    @{ Name = "test-nvme";                     Script = "scripts\test-nvme.ps1" },
    @{ Name = "test-xhci";                     Script = "scripts\test-xhci.ps1" },
    @{ Name = "test-compositor";               Script = "scripts\test-compositor.ps1" },
    @{ Name = "test-compositor-crash";         Script = "scripts\test-compositor-crash.ps1" },
    @{ Name = "test-input-routing";            Script = "scripts\test-input-routing.ps1" },
    @{ Name = "test-terminal";                 Script = "scripts\test-terminal.ps1" },
    @{ Name = "test-text-editor";              Script = "scripts\test-text-editor.ps1" },
    @{ Name = "test-manifest";                 Script = "scripts\test-manifest.ps1" },
    @{ Name = "test-installer";                Script = "scripts\test-installer.ps1" },
    @{ Name = "test-tcp-two-instance";         Script = "scripts\test-tcp-two-instance.ps1" },
    @{ Name = "test-e1000";                    Script = "scripts\test-e1000.ps1" },
    @{ Name = "test-shell";                    Script = "scripts\test-shell.ps1" },
    @{ Name = "test-release";                  Script = "scripts\test-release.ps1" }
)

$results = @()
$overallStart = Get-Date

foreach ($t in $suite) {
    Write-Output ""
    Write-Output "================================================================"
    Write-Output "=== Running $($t.Name) ==="
    Write-Output "================================================================"
    $start = Get-Date
    & powershell -ExecutionPolicy Bypass -File $t.Script
    $exitCode = $LASTEXITCODE
    $elapsed = (Get-Date) - $start
    $results += [PSCustomObject]@{
        Test       = $t.Name
        Result     = if ($exitCode -eq 0) { "PASS" } else { "FAIL" }
        ElapsedSec = [math]::Round($elapsed.TotalSeconds, 1)
    }
}

$overallElapsed = (Get-Date) - $overallStart

Write-Output ""
Write-Output "================================================================"
Write-Output "=== Full suite summary (docs/ROADMAP.md Phase 8 deliverable 3) ==="
Write-Output "================================================================"
$results | Format-Table -AutoSize | Out-String | Write-Output
Write-Output "Total wall-clock time: $([math]::Round($overallElapsed.TotalSeconds, 1))s"

$failed = $results | Where-Object { $_.Result -eq "FAIL" }
if ($failed.Count -eq 0) {
    Write-Output ""
    Write-Output "ALL $($results.Count) SUITES PASSED -- full automated suite green from a clean checkout, no manual steps."
    exit 0
} else {
    Write-Output ""
    Write-Error "$($failed.Count) of $($results.Count) suites FAILED: $($failed.Test -join ', ')"
    exit 1
}
