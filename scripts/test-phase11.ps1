# Master Test Suite for Phase 11:
# Hardware Breadth (Tier 3) and Power Management
#
# Covers:
#  - Item 11.1: Complete xHCI HID class drivers (keyboard and mouse report decoding into input_routing)
#  - Item 11.2: Dynamic USB hot-plug support (attach, detach, disable slot, dynamic rebinding)
#  - Item 11.3: ACPI power management (FADT parsing, CPU C-states, S3 suspend/resume with register/memory preservation)
#  - Item 11.4: Published, bounded Tier 3 hardware matrix (docs/SUPPORTED_HARDWARE.md and docs/TIER3_HARDWARE.md)

param(
    [switch]$SkipRegression = $false
)

$ErrorActionPreference = "Stop"
$repoRoot = "D:\Operating_System"
Set-Location $repoRoot
[Environment]::CurrentDirectory = $repoRoot

$cargoBin = "C:\Users\Gannoju Pavan\.cargo\bin"
if (Test-Path $cargoBin) {
    $env:PATH = "$cargoBin;$env:PATH"
}

$startTime = Get-Date

Write-Output "================================================================="
Write-Output "    AGENTIC OS -- PHASE 11 MASTER TEST SUITE"
Write-Output "    Hardware Breadth (Tier 3) and Power Management"
Write-Output "================================================================="
Write-Output ""

# 1. Verify Documentation Deliverables (Item 11.4)
Write-Output "=== Test 1: Tier 3 Documentation and Matrix Verification (Item 11.4) ==="
if (-not (Test-Path "docs\SUPPORTED_HARDWARE.md")) {
    Write-Error "docs\SUPPORTED_HARDWARE.md missing"
    exit 1
}
if (-not (Test-Path "docs\TIER3_HARDWARE.md")) {
    Write-Error "docs\TIER3_HARDWARE.md missing"
    exit 1
}
$suppHw = Get-Content "docs\SUPPORTED_HARDWARE.md" -Raw
$tier3Hw = Get-Content "docs\TIER3_HARDWARE.md" -Raw

$docChecks = @(
    "Dell OptiPlex",
    "Lenovo ThinkPad",
    "HP EliteDesk",
    "Intel VT-d",
    "xHCI",
    "ACPI"
)
foreach ($dc in $docChecks) {
    if (-not $suppHw.Contains($dc) -or -not $tier3Hw.Contains($dc)) {
        Write-Error "Documentation check failed for pattern: $dc"
        exit 1
    }
}
Write-Output "  PASS: Tier 3 hardware matrix verified in documentation"
Write-Output ""

# 2. xHCI USB Host Controller, HID Class Drivers, and Dynamic Hot-Plug (Items 11.1 and 11.2)
Write-Output "=== Test 2: xHCI HID Class Drivers and Hot-Plug (Items 11.1 and 11.2) ==="
& powershell -ExecutionPolicy Bypass -File "scripts\test-xhci.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "xHCI test suite failed"
    exit 1
}
Write-Output ""

# 3. ACPI Power Management, CPU C-States, and S3 Suspend/Resume (Item 11.3)
Write-Output "=== Test 3: ACPI Power Management and S3 Suspend/Resume (Item 11.3) ==="
& powershell -ExecutionPolicy Bypass -File "scripts\test-power.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "ACPI power management test suite failed"
    exit 1
}
Write-Output ""

# 4. Host-side unit tests
Write-Output "=== Test 4: Host-side Unit Tests ==="
& powershell -ExecutionPolicy Bypass -File "scripts\test-host.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Error "Host test suite failed"
    exit 1
}
Write-Output ""

# 5. Full 23-suite regression battery (optional, if not skipped)
if (-not $SkipRegression) {
    Write-Output "=== Test 5: Full Integration Regression Battery ==="
    & powershell -ExecutionPolicy Bypass -File "scripts\test-integration.ps1"
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Regression battery failed"
        exit 1
    }
}

$elapsed = [Math]::Round(((Get-Date) - $startTime).TotalSeconds, 1)
Write-Output ""
Write-Output "================================================================="
Write-Output "  ALL PHASE 11 DELIVERABLES FULLY VERIFIED (took ${elapsed}s)"
Write-Output "  - 11.1: Complete xHCI HID Class Drivers (Keyboard and Mouse)"
Write-Output "  - 11.2: Dynamic USB Hot-Plug Support (Attach / Detach / Unbind)"
Write-Output "  - 11.3: ACPI Power Management (C-States and S3 Suspend/Resume)"
Write-Output "  - 11.4: Published Tier 3 Hardware Matrix (Dell/Lenovo/HP)"
Write-Output "================================================================="
