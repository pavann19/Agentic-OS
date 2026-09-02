//! Phase 4's object store: capability-scoped naming for real files.
//! `docs/ROADMAP.md`'s exit criterion for this item is exact: "There is
//! no global namespace an unprivileged process can walk; a process sees
//! what it holds capabilities to." This module is deliberately thin —
//! the real enforcement isn't a new access-control layer bolted on top
//! of files, it's that `capability.rs`'s EXISTING `CapabilityTable::
//! resolve` already has the right shape for this: a process without a
//! capability at a given `CapId` gets `CapError::NoSuchCapability`,
//! the EXACT SAME error a made-up, never-granted index would produce.
//! There is no "does file X exist" query anywhere in this kernel to even
//! ask — only "resolve this capability I already hold," which is a
//! structurally different question. A `FileObject` is named by its real
//! ext2 inode number (`kernel_common::ext2::FILE_INODE`, for now — this
//! increment's filesystem only has the one file), never a path string;
//! the capability itself IS the name.

use crate::capability::{CapError, CapId, CapabilityTable, KernelObjectKind, Rights};

/// Mints a fresh capability naming the file at `inode`. Same "only the
/// party that already decided to grant it can create one" discipline
/// `driver.rs::create_mmio_capability` documents — nothing here lets a
/// process grant itself access to an inode it doesn't already have a
/// capability for.
pub fn create_file_capability(table: &mut CapabilityTable, inode: u32, rights: Rights) -> CapId {
    let object_id = crate::capability::create_object(KernelObjectKind::FileObject { inode });
    table.grant(object_id, rights)
}

#[derive(Debug)]
pub enum ObjectStoreError {
    Cap(CapError),
    WrongObjectKind,
}

/// The ONLY way to learn a file's real inode number from a capability
/// table: resolve a `CapId` the caller already holds. `required` is
/// checked the same way every other capability-gated operation in this
/// kernel checks rights — Rights::MAP is reused here as "may open this
/// object" (no dedicated FILE_READ/FILE_WRITE right exists yet; a real
/// per-operation rights split is real future work once this store grows
/// past one file, not pretended-away here).
pub fn resolve_to_inode(table: &CapabilityTable, cap_id: CapId, required: Rights) -> Result<u32, ObjectStoreError> {
    let cap = table.resolve(cap_id, required).map_err(ObjectStoreError::Cap)?;
    match crate::capability::object_kind(cap.object_id) {
        Some(KernelObjectKind::FileObject { inode }) => Ok(inode),
        _ => Err(ObjectStoreError::WrongObjectKind),
    }
}
