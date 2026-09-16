#![no_main]
use kernel_common::ext2;
use libfuzzer_sys::fuzz_target;

// read_file_data trusts the inode's own on-disk i_size field to decide
// how many bytes to copy out of the data block -- its own doc comment
// says this explicitly. That size field is exactly the kind of value a
// corrupt or foreign-formatted disk could set to anything; this target
// exists to prove `.min(BLOCK_SIZE).min(out.len())` actually holds for
// every possible on-disk value, not just the ones a real formatter
// would ever write. Also covers read_inode_entry and is_formatted,
// the other two raw-bytes-in parsers in this module.
fuzz_target!(|data: &[u8]| {
    if data.len() < 2 * ext2::BLOCK_SIZE {
        return;
    }
    let inode_table_block = &data[..ext2::BLOCK_SIZE];
    let data_block = &data[ext2::BLOCK_SIZE..2 * ext2::BLOCK_SIZE];

    let mut out = [0u8; ext2::BLOCK_SIZE];
    let n = ext2::read_file_data(inode_table_block, data_block, &mut out);
    assert!(n <= ext2::BLOCK_SIZE);
    assert!(n <= out.len());

    let _ = ext2::is_formatted(inode_table_block);

    for inode in [1u32, ext2::ROOT_INODE, ext2::FILE_INODE, 12, 255] {
        let info = ext2::read_inode_entry(inode_table_block, inode);
        // Fields are fixed-width reads off a fixed-size block --
        // decoding can never itself run off the end of `table_block`.
        // What's real here is confirming the DECODED values are still
        // usable without a caller needing its own extra bounds check:
        // `direct_blocks` is always exactly 12 entries regardless of
        // what garbage was on disk.
        assert_eq!(info.direct_blocks.len(), 12);
    }
});
