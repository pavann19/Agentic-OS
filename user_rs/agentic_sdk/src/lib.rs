//! Phase 13 deliverable 3's first real slice: the raw syscall wrappers
//! and COM1 I/O helpers every `user_rs/*_driver` crate has been
//! re-typing by hand since Phase 3 — `serial_driver`, `ahci_driver`,
//! `compositor_driver`, `window_client_driver`, `netstack_driver`, and
//! others all carry their own byte-identical (or near-identical, and in
//! at least one documented case, ONE fixed a real clobber-list bug the
//! others still carry — see `syscall::syscall3`'s own doc) copies of
//! `outb`/`inb`/`write_str` and a growing set of `syscallN` wrappers.
//! This crate is a single, reusable `#![no_std]` library other apps
//! depend on instead of copy-pasting, the same "document and reuse
//! rather than re-derive" discipline `installer.rs` just applied to the
//! spawn-side of this exact duplication.
//!
//! Real, disclosed scope: this is the syscall-wrapper/I/O layer only.
//! The per-app `.cargo/config.toml` / `linker.ld` / `build.rs` trio
//! still has to exist in every app's own directory — cargo has no
//! mechanism for a path dependency to inject build/link configuration
//! into its dependent's own package. See `docs/SDK.md` for exactly
//! what those three files contain and why, so a genuinely new app can
//! be built without reading kernel source, per this deliverable's own
//! exit-criterion wording.
#![no_std]

// Real bug found bringing this crate up: this module's obvious name
// (`com1.rs`) is a reserved Windows device name (COM1-COM9, LPT1-9,
// CON, PRN, AUX, NUL, with or without an extension) -- the file itself
// created and read back fine through .NET/PowerShell APIs, but `git
// add` failed with a plain "No such file or directory": git's own
// lower-level file open resolves a bare `com1.rs` to the actual serial
// port device object, not a disk file, on this Windows host. Named
// `serial_com1` instead; the module's own public API (`write_str`,
// `write_dec_u64`) is unaffected, only the file/module name changed.
pub mod file_service; // real block/file-I/O-for-apps path (IPC-mediated), see its own module doc
pub mod serial_com1;
pub mod surface; // Phase 12 deliverable 4 -- real PSF1 text rendering via SYS_SURFACE_DRAW_TEXT
pub mod syscall;
pub mod text_widget; // Phase 12 deliverable 4 -- the one real, minimal scrollable-text-region widget

pub use serial_com1 as com1;
