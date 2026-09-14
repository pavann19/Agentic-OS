//! Phase 13 deliverable 5 & deliverable 2:
//! Package store, on-disk package format (`AGYPKG1`), and reproducible-build
//! update verification.
//!
//! Standard binary package structure:
//!   - `PackageHeader` (64 bytes):
//!       - magic: `b"AGYPKG1\0"`
//!       - version: u32
//!       - manifest_mask: u16 (bitmask of allowed `CapKind`s)
//!       - flags: u16
//!       - elf_offset: u32 (byte offset where ELF binary starts)
//!       - elf_size: u32 (size of ELF binary in bytes)
//!       - checksum: u32 (Adler-32 checksum of ELF binary)
//!       - name: [u8; 32] (null-terminated application name)
//!   - ELF binary payload at `elf_offset`
//!
//! Enforces Phase 8's reproducible-build checksum discipline:
//!   - A package is validated before installation (magic, offsets, checksum).
//!   - An application update is rejected if:
//!       a) Its checksum is corrupted or does not match the ELF bytes.
//!       b) Its checksum does not match the expected hash from a reproducible
//!          clean build.
//!       c) Its version does not strictly increase.

use crate::installer::{self, CapRequest};
use crate::klog_info;
use crate::manifest::Manifest;

pub const PACKAGE_MAGIC: [u8; 8] = *b"AGYPKG1\0";

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PackageHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub manifest_mask: u16,
    pub flags: u16,
    pub elf_offset: u32,
    pub elf_size: u32,
    pub checksum: u32,
    pub name: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum PackageError {
    TooShort,
    BadMagic,
    InvalidOffsets,
    ChecksumMismatch { expected: u32, actual: u32 },
    UpdateVersionDowngrade { old_version: u32, new_version: u32 },
}

pub struct Package<'a> {
    pub header: PackageHeader,
    pub elf: &'a [u8],
}

/// Standard Adler-32 checksum (RFC 1950): fast, deterministic,
/// zero-dependency, and verified on host and target.
pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Parses and validates package integrity from a byte buffer (e.g. read
/// from on-disk store or memory).
pub fn parse_and_verify(bytes: &[u8]) -> Result<Package<'_>, PackageError> {
    let hdr_size = core::mem::size_of::<PackageHeader>();
    if bytes.len() < hdr_size {
        return Err(PackageError::TooShort);
    }
    let hdr: PackageHeader = unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const PackageHeader) };
    if hdr.magic != PACKAGE_MAGIC {
        klog_info!("PACKAGE_BAD_MAGIC");
        return Err(PackageError::BadMagic);
    }
    let start = hdr.elf_offset as usize;
    let end = start.saturating_add(hdr.elf_size as usize);
    if start < hdr_size || end > bytes.len() {
        klog_info!("PACKAGE_INVALID_OFFSETS start={} end={} total={}", start, end, bytes.len());
        return Err(PackageError::InvalidOffsets);
    }
    let elf = &bytes[start..end];
    let computed = adler32(elf);
    if computed != hdr.checksum {
        klog_info!("PACKAGE_CHECKSUM_MISMATCH expected=0x{:08x} actual=0x{:08x}", hdr.checksum, computed);
        return Err(PackageError::ChecksumMismatch {
            expected: hdr.checksum,
            actual: computed,
        });
    }
    klog_info!(
        "PACKAGE_VERIFY_OK version={} size={} checksum=0x{:08x}",
        hdr.version,
        hdr.elf_size,
        computed
    );
    Ok(Package { header: hdr, elf })
}

/// Verifies an application update against declared source checksum and
/// version monotonicity.
pub fn verify_update<'a>(
    current_version: u32,
    new_pkg_bytes: &'a [u8],
    expected_source_checksum: u32,
) -> Result<Package<'a>, PackageError> {
    let pkg = parse_and_verify(new_pkg_bytes)?;
    if pkg.header.version <= current_version {
        klog_info!(
            "PACKAGE_UPDATE_REJECTED: version downgrade or unchanged old={} new={}",
            current_version,
            pkg.header.version
        );
        return Err(PackageError::UpdateVersionDowngrade {
            old_version: current_version,
            new_version: pkg.header.version,
        });
    }
    if pkg.header.checksum != expected_source_checksum {
        klog_info!(
            "PACKAGE_UPDATE_REJECTED: not reproducible against declared source expected=0x{:08x} actual=0x{:08x}",
            expected_source_checksum,
            pkg.header.checksum
        );
        return Err(PackageError::ChecksumMismatch {
            expected: expected_source_checksum,
            actual: pkg.header.checksum,
        });
    }
    klog_info!(
        "PACKAGE_UPDATE_ACCEPTED: version upgraded from {} to {} checksum=0x{:08x}",
        current_version,
        pkg.header.version,
        pkg.header.checksum
    );
    Ok(pkg)
}

/// Installs an app from its package bytes into the current thread's context,
/// enforcing the package's embedded capability manifest.
pub fn install_package_into_current_thread(
    pkg_bytes: &[u8],
    stack_vaddr: u64,
    requests: &[CapRequest],
) -> Option<(u64, u64)> {
    let pkg = match parse_and_verify(pkg_bytes) {
        Ok(p) => p,
        Err(e) => {
            klog_info!("PACKAGE_INSTALL_REJECTED {:?}", e);
            return None;
        }
    };
    let manifest = Manifest(pkg.header.manifest_mask);
    installer::install_into_current_thread(pkg.elf, stack_vaddr, manifest, requests)
}

/// Live adversarial demo for app update & reproducibility check
pub fn run_app_update_demo() {
    klog_info!("APP_UPDATE_DEMO_START");

    // Synthesize a test ELF binary payload
    let elf_v1: [u8; 64] = [0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 42, 43, 44, 45, 0, 0, 0, 0,
                            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let checksum_v1 = adler32(&elf_v1);
    klog_info!("APP_UPDATE_V1_CHECKSUM=0x{:08x}", checksum_v1);

    // Build package v1
    let mut pkg_v1 = [0u8; 128];
    let hdr_v1 = PackageHeader {
        magic: PACKAGE_MAGIC,
        version: 1,
        manifest_mask: 0x0101, // Surface (bit 8) | IpcEndpoint (bit 0)
        flags: 0,
        elf_offset: 64,
        elf_size: 64,
        checksum: checksum_v1,
        name: *b"demo_app\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    };
    unsafe {
        core::ptr::write_unaligned(pkg_v1.as_mut_ptr() as *mut PackageHeader, hdr_v1);
        core::ptr::copy_nonoverlapping(elf_v1.as_ptr(), pkg_v1.as_mut_ptr().add(64), 64);
    }

    // 1. Verify v1 package parses and passes integrity
    match parse_and_verify(&pkg_v1) {
        Ok(_) => klog_info!("APP_UPDATE_V1_VERIFY_PASS"),
        Err(_) => klog_info!("APP_UPDATE_V1_VERIFY_FAIL"),
    }

    // 2. Build package v2 (updated version)
    let mut elf_v2 = elf_v1;
    elf_v2[10] = 99; // updated code
    let checksum_v2 = adler32(&elf_v2);
    klog_info!("APP_UPDATE_V2_CHECKSUM=0x{:08x}", checksum_v2);

    let mut pkg_v2 = [0u8; 128];
    let hdr_v2 = PackageHeader {
        magic: PACKAGE_MAGIC,
        version: 2,
        manifest_mask: 0x0101,
        flags: 0,
        elf_offset: 64,
        elf_size: 64,
        checksum: checksum_v2,
        name: *b"demo_app\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
    };
    unsafe {
        core::ptr::write_unaligned(pkg_v2.as_mut_ptr() as *mut PackageHeader, hdr_v2);
        core::ptr::copy_nonoverlapping(elf_v2.as_ptr(), pkg_v2.as_mut_ptr().add(64), 64);
    }

    // Verify clean v2 update succeeds against expected reproducible hash
    match verify_update(1, &pkg_v2, checksum_v2) {
        Ok(_) => klog_info!("APP_UPDATE_V2_ACCEPTED_PASS"),
        Err(_) => klog_info!("APP_UPDATE_V2_ACCEPTED_FAIL"),
    }

    // 3. Adversarial Check 1: Tampered package (bit flip in payload)
    let mut pkg_tampered = pkg_v2;
    pkg_tampered[70] ^= 0xFF; // tamper with ELF instruction
    match parse_and_verify(&pkg_tampered) {
        Err(PackageError::ChecksumMismatch { .. }) => {
            klog_info!("APP_UPDATE_TAMPERED_PAYLOAD_REJECTED_PASS");
        }
        _ => klog_info!("APP_UPDATE_TAMPERED_PAYLOAD_LEAKED_FAIL"),
    }

    // 4. Adversarial Check 2: Non-reproducible update (checksum does not match declared source)
    let wrong_source_checksum = 0xDEAD_BEEF;
    match verify_update(1, &pkg_v2, wrong_source_checksum) {
        Err(PackageError::ChecksumMismatch { .. }) => {
            klog_info!("APP_UPDATE_NON_REPRODUCIBLE_REJECTED_PASS");
        }
        _ => klog_info!("APP_UPDATE_NON_REPRODUCIBLE_LEAKED_FAIL"),
    }

    // 5. Adversarial Check 3: Version downgrade attack (version 1 offered as update to version 2)
    match verify_update(2, &pkg_v1, checksum_v1) {
        Err(PackageError::UpdateVersionDowngrade { .. }) => {
            klog_info!("APP_UPDATE_DOWNGRADE_REJECTED_PASS");
        }
        _ => klog_info!("APP_UPDATE_DOWNGRADE_LEAKED_FAIL"),
    }

    klog_info!("APP_UPDATE_DEMO_DONE");
}
