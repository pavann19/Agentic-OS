# Regenerates user_rs/virtio_net_driver/variants/{virtio_net_driver_good,
# virtio_net_driver_induced_fault} -- kernel_rs/src/virtio_net.rs embeds
# BOTH via `include_bytes!`, chosen at kernel build time by the
# `synthesis_induced_fault` kernel Cargo feature. Not committed (same
# gitignore convention as every other user_rs/*/target/ build output in
# this repo) -- run this before building kernel_rs if `variants/` is
# missing or stale, then `cargo build --release` (normal kernel) and
# `cargo build --release --features synthesis_induced_fault` (the
# induced-fault kernel, see scripts/test-synthesis-fault-demo.ps1).

$ErrorActionPreference = "Stop"

Push-Location user_rs\virtio_net_driver
try {
    New-Item -ItemType Directory -Force -Path variants | Out-Null

    Write-Output "Building normal (fixed) virtio_net_driver..."
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "normal build failed" }
    Copy-Item -Force target\x86_64-unknown-none\release\virtio_net_driver variants\virtio_net_driver_good

    Write-Output "Building induced-fault virtio_net_driver..."
    cargo build --release --features induced_fault
    if ($LASTEXITCODE -ne 0) { throw "induced_fault build failed" }
    Copy-Item -Force target\x86_64-unknown-none\release\virtio_net_driver variants\virtio_net_driver_induced_fault

    Write-Output "Done: variants\virtio_net_driver_good, variants\virtio_net_driver_induced_fault"
} finally {
    Pop-Location
}
