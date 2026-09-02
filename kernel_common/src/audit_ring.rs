//! Phase 4's last item: audit log persistence, with rotation (a
//! deferred obligation from ADR-005, `docs/ROADMAP.md`). A real,
//! minimal on-disk ring buffer -- not the full in-memory audit trail
//! `audit.rs` already keeps (that would need cross-process IPC plumbing
//! from every capability operation, kernel-side, to the ring-3 block
//! driver that owns real disk access -- real, separate future work, not
//! attempted here); this persists `virtio_blk_driver`'s OWN real
//! actions (format/write/read/self-check) as real, rotating, on-disk
//! audit records, proven to survive a real reboot the same way the
//! filesystem itself was (see `ext2.rs`'s own module doc for that
//! proof's shape).
//!
//! Real, on-disk, spec-of-its-own-devising format (there's no existing
//! standard "audit ring" format the way ext2 is a standard filesystem
//! format -- this is a small, honest, from-scratch design, documented
//! here in full): one header block, `RING_SLOTS` record blocks after
//! it, each 1024 bytes like `ext2::BLOCK_SIZE` (deliberately the SAME
//! block size, so this can live on the SAME disk right after the
//! filesystem's own fixed footprint without any unit-conversion
//! surprises).
//!
//! Rotation is real, not simulated: `next_write_index` wraps modulo
//! `RING_SLOTS`, so the `RING_SLOTS + 1`th record genuinely overwrites
//! the 1st slot's bytes on disk -- `total_written_count` (monotonically
//! increasing, itself persisted) is what lets a reader tell "rotation
//! has genuinely begun" (`total_written_count > RING_SLOTS`) from "the
//! ring isn't even full yet".

pub const BLOCK_SIZE: usize = 1024;
pub const MAGIC: u32 = 0xA0D170C5; // "AUDIT LOGS" -ish, arbitrary but fixed
pub const RING_SLOTS: u32 = 4;
pub const MAX_MESSAGE_LEN: usize = 512;

fn wu32(b: &mut [u8], off: usize, v: u32) {
    unsafe { core::ptr::write_volatile(&mut b[off], (v & 0xFF) as u8) };
    unsafe { core::ptr::write_volatile(&mut b[off + 1], ((v >> 8) & 0xFF) as u8) };
    unsafe { core::ptr::write_volatile(&mut b[off + 2], ((v >> 16) & 0xFF) as u8) };
    unsafe { core::ptr::write_volatile(&mut b[off + 3], ((v >> 24) & 0xFF) as u8) };
}
fn wu64(b: &mut [u8], off: usize, v: u64) {
    for i in 0..8 {
        unsafe { core::ptr::write_volatile(&mut b[off + i], ((v >> (i * 8)) & 0xFF) as u8) };
    }
}
fn ru32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn ru64(b: &[u8], off: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[off..off + 8]);
    u64::from_le_bytes(a)
}

/// True if `block` (the real header block) is a valid, already-
/// initialized ring -- the real magic-number check every reader does
/// first, same discipline as `ext2::is_formatted`.
pub fn is_initialized(header_block: &[u8]) -> bool {
    ru32(header_block, 0) == MAGIC
}

/// Real header fields: which slot the NEXT record goes into, and how
/// many records have EVER been written (the real, persisted rotation
/// counter -- not reset by wrapping, so `total_written_count` alone
/// tells a reader whether rotation has genuinely started).
pub struct RingHeader {
    pub next_write_index: u32,
    pub total_written_count: u64,
}

pub fn read_header(header_block: &[u8]) -> RingHeader {
    RingHeader {
        next_write_index: ru32(header_block, 4),
        total_written_count: ru64(header_block, 8),
    }
}

pub fn build_header(b: &mut [u8], h: &RingHeader) {
    for i in 0..b.len() {
        unsafe { core::ptr::write_volatile(&mut b[i], 0) };
    }
    wu32(b, 0, MAGIC);
    wu32(b, 4, h.next_write_index);
    wu64(b, 8, h.total_written_count);
}

/// Builds one real record block: a sequence number (the record's own
/// `total_written_count` at the time it was written -- survives
/// rotation identifying THIS record even after its slot is later
/// reused) plus a short message.
pub fn build_record(b: &mut [u8], seq: u64, message: &[u8]) {
    for i in 0..b.len() {
        unsafe { core::ptr::write_volatile(&mut b[i], 0) };
    }
    wu64(b, 0, seq);
    let len = message.len().min(MAX_MESSAGE_LEN);
    wu32(b, 8, len as u32);
    for i in 0..len {
        unsafe { core::ptr::write_volatile(&mut b[12 + i], message[i]) };
    }
}

/// Reads a record block back: real sequence number, real message bytes
/// copied into `out`, returns the real byte count (trusts the record's
/// own persisted length, not a caller-assumed one -- same discipline
/// `ext2::read_file_data` documents for the same reason).
pub fn read_record(record_block: &[u8], out: &mut [u8]) -> (u64, usize) {
    let seq = ru64(record_block, 0);
    let len = (ru32(record_block, 8) as usize).min(MAX_MESSAGE_LEN).min(out.len());
    for i in 0..len {
        unsafe { core::ptr::write_volatile(&mut out[i], record_block[12 + i]) };
    }
    (seq, len)
}
