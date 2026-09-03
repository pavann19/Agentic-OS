# Phase 8, deliverable 2: real release image build.
#
# Scope, stated honestly: this project has no FAT32-image-formatting
# tool readily available on Windows without WSL (`mkfs.vfat` isn't
# present), so "release image" here means the real, complete
# deployable artifact SET -- the exact same files
# `scripts/test-boot.ps1`'s own `-drive file=fat:rw:...` overlay boots
# from -- assembled into one clean output directory with a real
# manifest (SHA256 of every artifact), not a raw .img file. Anyone
# with `mkfs.vfat`/`dd` (or QEMU's own fat:rw: overlay, as this
# project's own test scripts already use) can turn this directory into
# a real bootable disk image directly; the byte-for-byte CONTENT is
# what's actually being verified as reproducible here, not a
# particular container format around it.
#
# Always builds from a CLEAN state (`cargo clean` first) -- a release
# build reusing incremental artifacts from unrelated prior work isn't
# a real release build.

param(
    [string]$OutDir = "dist\agentic-os-release"
)

$ErrorActionPreference = "Stop"

function Get-Sha256($path) {
    (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLower()
}

Write-Output "=== Building boot_rs (UEFI bootloader) from clean ==="
Push-Location boot_rs
try {
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    cargo clean *>&1 | Out-Null
    $ErrorActionPreference = $prevEap
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "boot_rs release build failed" }
} finally {
    Pop-Location
}

Write-Output "=== Building kernel_rs from clean ==="
Push-Location kernel_rs
try {
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    cargo clean *>&1 | Out-Null
    $ErrorActionPreference = $prevEap
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "kernel_rs release build failed" }
} finally {
    Pop-Location
}

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Force -Path "$OutDir\EFI\BOOT" | Out-Null

Copy-Item "boot_rs\target\x86_64-unknown-uefi\release\agentic_bootloader.efi" "$OutDir\EFI\BOOT\BOOTX64.EFI"
Copy-Item "kernel_rs\target\x86_64-unknown-none\release\agentic_kernel" "$OutDir\kernel.elf"
Copy-Item "boot_rs\qemu_fatdir\font.psf" "$OutDir\font.psf"

$manifest = [ordered]@{
    built_at_utc = (Get-Date).ToUniversalTime().ToString("o")
    artifacts    = [ordered]@{
        "EFI/BOOT/BOOTX64.EFI" = Get-Sha256 "$OutDir\EFI\BOOT\BOOTX64.EFI"
        "kernel.elf"           = Get-Sha256 "$OutDir\kernel.elf"
        "font.psf"             = Get-Sha256 "$OutDir\font.psf"
    }
}
$manifest | ConvertTo-Json | Set-Content "$OutDir\MANIFEST.json"

Write-Output ""
Write-Output "=== Release artifacts (real SHA256, not simulated) ==="
$manifest.artifacts.GetEnumerator() | ForEach-Object { Write-Output "  $($_.Key): $($_.Value)" }
Write-Output ""
Write-Output "Release build complete: $OutDir"
