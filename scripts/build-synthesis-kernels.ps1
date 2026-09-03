# Regenerates boot_rs\kernel_variants\{kernel-good.elf,
# kernel-induced-fault.elf} -- the two kernel builds
# scripts/test-synthesis-fault-demo.ps1 boots in sequence. Not
# committed (same gitignore convention as every other build output in
# this repo). Run scripts/build-virtio-net-variants.ps1 FIRST -- these
# kernel builds embed its output.

$ErrorActionPreference = "Stop"

New-Item -ItemType Directory -Force -Path boot_rs\kernel_variants | Out-Null

Push-Location kernel_rs
try {
    Write-Output "Building normal (fixed) kernel..."
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "normal kernel build failed" }
    Copy-Item -Force target\x86_64-unknown-none\release\agentic_kernel ..\boot_rs\kernel_variants\kernel-good.elf

    Write-Output "Building induced-fault kernel..."
    cargo build --release --features synthesis_induced_fault
    if ($LASTEXITCODE -ne 0) { throw "induced-fault kernel build failed" }
    Copy-Item -Force target\x86_64-unknown-none\release\agentic_kernel ..\boot_rs\kernel_variants\kernel-induced-fault.elf

    Write-Output "Done: boot_rs\kernel_variants\kernel-good.elf, kernel-induced-fault.elf"
} finally {
    Pop-Location
}

# Restore the normal build as the deployed kernel.elf, so a plain
# `cargo build` right after this script leaves boot_rs\qemu_fatdir in
# its usual state for test-boot.ps1/etc.
Copy-Item -Force boot_rs\kernel_variants\kernel-good.elf boot_rs\qemu_fatdir\kernel.elf
