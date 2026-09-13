# Real host-side (non-QEMU) test suite -- kernel_common's own data
# structures (ext2, audit_ring, page-table math) verified directly on
# the host, no emulator involved. Thin wrapper around `cargo test` so
# this project's test-*.ps1 naming convention covers it too (matches
# docs/ROADMAP.md Phase 8 deliverable 3's own list: "test-boot,
# test-host, test-faults, test-integration, test-release").

$ErrorActionPreference = "Stop"

Push-Location host_tests
try {
    cargo test --release --lib
    if ($LASTEXITCODE -ne 0) { throw "host_tests failed" }
} finally {
    Pop-Location
}
