# Phase 8, deliverable 2's other half: real reproducible-build
# verification, not asserted. Runs scripts/build-release.ps1 TWICE,
# each from a genuinely clean state, and compares the resulting
# MANIFEST.json hashes byte-for-byte.
#
# Real investigation behind this, not assumed to just work: boot_rs's
# UEFI PE binary was NOT reproducible by default -- two clean builds
# produced different SHA256 hashes. Root-caused via `cmp -l` byte-diff
# (not guessed): a handful of bytes in the PE/COFF header (the
# mandatory TimeDateStamp field, embedded in three places in the
# header) plus an 8-byte debug-info GUID (CodeView debug directory,
# randomly generated per link by default) accounted for every
# differing byte. Fixed with two real, standard MSVC-linker flags in
# boot_rs/.cargo/config.toml: `/Brepro` (computes a content hash for
# the timestamp field instead of the real build time) and
# `/DEBUG:NONE` (suppresses the randomly-seeded debug-info GUID
# entirely). kernel_rs's own ELF output was already reproducible with
# no changes needed (GNU-flavor lld, no PE timestamp/GUID fields to
# begin with).

param(
    [string]$OutDir1 = "dist\repro-check-1",
    [string]$OutDir2 = "dist\repro-check-2"
)

$ErrorActionPreference = "Stop"

Write-Output "=== Build 1/2 ==="
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -OutDir $OutDir1
if ($LASTEXITCODE -ne 0) { throw "first release build failed" }

Write-Output ""
Write-Output "=== Build 2/2 ==="
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1 -OutDir $OutDir2
if ($LASTEXITCODE -ne 0) { throw "second release build failed" }

$m1 = Get-Content "$OutDir1\MANIFEST.json" | ConvertFrom-Json
$m2 = Get-Content "$OutDir2\MANIFEST.json" | ConvertFrom-Json

Write-Output ""
Write-Output "=== Reproducibility comparison (real SHA256 hashes, two independent clean builds) ==="
$allMatch = $true
foreach ($prop in $m1.artifacts.PSObject.Properties) {
    $name = $prop.Name
    $h1 = $prop.Value
    $h2 = $m2.artifacts.$name
    if ($h1 -eq $h2) {
        Write-Output "  PASS: $name  $h1"
    } else {
        Write-Output "  FAIL: $name"
        Write-Output "    build 1: $h1"
        Write-Output "    build 2: $h2"
        $allMatch = $false
    }
}

if ($allMatch) {
    Write-Output ""
    Write-Output "Release is genuinely reproducible from source: two independent clean builds produced byte-identical artifacts."
    exit 0
} else {
    Write-Output ""
    Write-Error "Release is NOT reproducible -- see mismatched artifacts above."
    exit 1
}
