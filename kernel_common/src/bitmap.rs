//! PMM bitmap bit-twiddling — the exact logic `kernel_rs/src/pmm.rs`'s
//! `bitmap_set`/`bitmap_clear`/`bitmap_test` used, extracted here to
//! operate on a plain `&mut [u8]` instead of a raw physical-pointer
//! translation, so it's testable on the host with zero unsafe.

pub fn set(bitmap: &mut [u8], index: u64) {
    bitmap[(index / 8) as usize] |= 1 << (index % 8);
}

pub fn clear(bitmap: &mut [u8], index: u64) {
    bitmap[(index / 8) as usize] &= !(1 << (index % 8));
}

pub fn test(bitmap: &[u8], index: u64) -> bool {
    (bitmap[(index / 8) as usize] & (1 << (index % 8))) != 0
}

/// The page-count-and-rounding math from `pmm::reserve_range` — given a
/// possibly-unaligned `[start, start+length)` byte range, returns
/// `(start_page, page_count)` covering it, rounding OUTWARD so a range
/// that starts or ends mid-page still gets that whole page included.
pub fn reserve_range_pages(start: u64, length: u64, page_size: u64) -> (u64, u64) {
    let start_page = start / page_size;
    let mut pages = (length + page_size - 1) / page_size;
    if start % page_size != 0 {
        pages = (length + (start % page_size) + page_size - 1) / page_size;
    }
    (start_page, pages)
}
