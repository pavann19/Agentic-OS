#![no_main]
use kernel_common::ext2;
use libfuzzer_sys::fuzz_target;

// resolve_path walks a chain of real disk reads keyed by inode number,
// driven by a caller-supplied path string that -- in the real kernel --
// comes straight off the wire in an FS_OP_LOOKUP/FS_OP_CREATE request
// (`user_rs/virtio_blk_driver/src/main.rs`'s `driver_resolve_path`).
// This target builds a small synthetic multi-block "disk" out of the
// fuzz input and drives resolve_path with an arbitrary path string
// against it, asserting only that it terminates and never panics --
// the same discipline `ext2_dir_parsing.rs` applies to a single block.
const NUM_BLOCKS: usize = 4;

fuzz_target!(|data: &[u8]| {
    let needed = 2 + NUM_BLOCKS * ext2::BLOCK_SIZE;
    if data.len() < needed {
        return;
    }
    let path_len = (u16::from_le_bytes([data[0], data[1]]) as usize) % 256;
    let path_bytes = &data[2..2 + path_len.min(data.len() - 2)];
    let path = match core::str::from_utf8(path_bytes) {
        Ok(s) => s,
        Err(_) => return,
    };

    let disk_start = 2 + 256; // fixed offset regardless of actual path_len, keeps block data stable across similar inputs
    if data.len() < disk_start + NUM_BLOCKS * ext2::BLOCK_SIZE {
        return;
    }
    let mut blocks = [[0u8; ext2::BLOCK_SIZE]; NUM_BLOCKS];
    for (i, b) in blocks.iter_mut().enumerate() {
        let off = disk_start + i * ext2::BLOCK_SIZE;
        b.copy_from_slice(&data[off..off + ext2::BLOCK_SIZE]);
    }

    let mut reads = 0u32;
    let result = ext2::resolve_path(path, ext2::ROOT_INODE, |inode, buf| {
        reads += 1;
        // A real, malicious/corrupt directory chain that keeps
        // resolving to a "valid" next block forever would hang the
        // driver -- resolve_path itself must terminate in at most
        // one read per path component (PathComponentIterator can't
        // produce more components than the input is long), which this
        // bound checks directly rather than trusting a fuzzer timeout
        // to notice.
        assert!(reads as usize <= path.len() + 1);
        let idx = (inode as usize) % NUM_BLOCKS;
        buf.copy_from_slice(&blocks[idx]);
        true
    });
    let _ = result;
});
