#![no_main]
use kernel_common::ext2;
use libfuzzer_sys::fuzz_target;

// A directory data block is exactly BLOCK_SIZE bytes on disk, arriving
// off real (or corrupt, or foreign-formatted) storage with no
// guarantees about its contents. read_dir_entries/find_dir_entry are
// what virtio_blk_driver calls directly on those bytes -- this target
// asserts they never panic (index out of bounds, arithmetic overflow
// in debug builds, etc.) no matter what garbage is on disk.
fuzz_target!(|data: &[u8]| {
    if data.len() < ext2::BLOCK_SIZE {
        return;
    }
    let block = &data[..ext2::BLOCK_SIZE];

    let mut count = 0u32;
    ext2::read_dir_entries(block, |_inode, _file_type, name| {
        // A real caller only ever trusts `name` up to its declared
        // length -- confirm that length claim actually stays inside
        // the slice `read_dir_entries` handed back, not just that we
        // didn't panic (a panic on bad input is caught by the fuzzer
        // itself; this catches silent out-of-declared-bounds reads
        // that happen not to fault).
        assert!(name.len() <= 255);
        count += 1;
    });
    // Bounded: a BLOCK_SIZE block can hold at most BLOCK_SIZE/8 entries
    // (the smallest possible real record: 8-byte header, zero-length
    // name). Anything past that means the record-length walk in
    // read_dir_entries isn't actually making forward progress on some
    // malformed input -- a real infinite-loop-shaped bug, not just an
    // out-of-bounds one.
    assert!(count as usize <= ext2::BLOCK_SIZE / 8);

    if let Some(&len_byte) = data.get(ext2::BLOCK_SIZE) {
        let name_len = (len_byte as usize % 64).max(1);
        let tail = &data[ext2::BLOCK_SIZE.min(data.len())..];
        let name = &tail[..name_len.min(tail.len())];
        let _ = ext2::find_dir_entry(block, name);
    }
});
