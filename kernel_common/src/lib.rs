//! Pure, hardware-independent kernel logic — see Cargo.toml's comment for
//! why this is a separate crate. `#![no_std]` but not bare-metal-specific:
//! no inline asm, no raw physical-memory access, nothing that requires
//! actually running on the target. Every function here operates on
//! caller-provided slices/values only.
#![no_std]

pub mod bitmap;
pub mod pagetable;

/// Rounds `addr` up to the nearest multiple of `align` (`align` must be a
/// power of two — matches every caller's usage in this codebase, and this
/// is deliberately not defensive against a non-power-of-two `align`, same
/// as the standard library's own `Layout` requires).
pub fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

/// Rounds `bytes` up to a whole number of `page_size`-sized pages.
pub fn pages_for(bytes: u64, page_size: u64) -> u64 {
    (bytes + page_size - 1) / page_size
}
