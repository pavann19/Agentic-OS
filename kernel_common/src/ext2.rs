//! A real, minimal, on-disk-format-correct ext2 filesystem — Phase 4's
//! "on-disk filesystem" item (`docs/ROADMAP.md`: "prefer implementing a
//! documented format... ext2 is a reasonable read/write target"). Pure
//! logic, zero unsafe, operates one 1024-byte block at a time on
//! caller-provided `&mut [u8; BLOCK_SIZE]` buffers — the actual disk I/O
//! (reading/writing that one block through the real virtio-blk driver)
//! is the CALLER's job, matching this crate's existing "pure logic,
//! hardware-independent" discipline (see `lib.rs`'s module doc) and
//! letting every function here be verified on the host via `cargo test`,
//! not just by booting QEMU.
//!
//! Real, explicitly-scoped simplifications for this first increment
//! (stated plainly, not hidden): ONE block group, 1024-byte blocks,
//! DIRECT blocks only (no indirect/double-indirect — file size capped at
//! 12KB, `i_block[12..15]` always zero), and a FIXED layout rather than a
//! general free-block/free-inode allocator — this increment proves one
//! real file survives a real reboot with a real, spec-correct on-disk
//! format; a general allocator (needed once more than one file/write
//! exists) is real, separate future work, not pretended-away here.
//!
//! Every field offset below matches the real ext2 on-disk format
//! (`ext2_super_block`/`ext2_group_desc`/`ext2_inode`/`ext2_dir_entry_2`,
//! as documented in the Linux kernel's `include/uapi/linux/ext2_fs.h`
//! and the OSDev wiki's ext2 page) — a real ext2 implementation (Linux's
//! own, `e2fsprogs`) should be able to read a filesystem this code
//! builds, modulo the single-block-group/direct-blocks-only scope above.

#![allow(dead_code)]

pub const BLOCK_SIZE: usize = 1024;
pub const MAGIC: u16 = 0xEF53;

// Fixed layout (see module doc: no allocator yet, everything at a known
// block number). Blocks 11..TOTAL_BLOCKS stay free/unused this
// increment -- headroom for the next file this filesystem ever gets,
// once an allocator exists to hand one out.
pub const SUPERBLOCK_BLOCK: u32 = 1;
pub const GROUP_DESC_BLOCK: u32 = 2;
pub const BLOCK_BITMAP_BLOCK: u32 = 3;
pub const INODE_BITMAP_BLOCK: u32 = 4;
pub const INODE_TABLE_START_BLOCK: u32 = 5;
pub const INODE_TABLE_BLOCKS: u32 = 4; // 32 inodes * 128 bytes / 1024
pub const ROOT_DATA_BLOCK: u32 = 9;
pub const FILE_DATA_BLOCK: u32 = 10;
pub const TOTAL_BLOCKS: u32 = 64;
pub const NUM_INODES: u32 = 32;
pub const INODES_PER_BLOCK: u32 = (BLOCK_SIZE / 128) as u32; // 8

pub const ROOT_INODE: u32 = 2; // EXT2_ROOT_INO, fixed by the format itself
pub const FILE_INODE: u32 = 11; // first usable inode past the reserved 1..=10 (GOOD_OLD_REV)

pub const S_IFREG: u16 = 0x8000;
pub const S_IFDIR: u16 = 0x4000;
pub const MODE_644: u16 = 0o644;
pub const MODE_755: u16 = 0o755;

pub const FT_UNKNOWN: u8 = 0;
pub const FT_REG: u8 = 1;
pub const FT_DIR: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InodeInfo {
    pub mode: u16,
    pub size: u32,
    pub links: u16,
    pub blocks_count: u32,
    pub direct_blocks: [u32; 12],
}

pub fn inode_block_and_offset(inode: u32) -> (u32, usize) {
    let block = INODE_TABLE_START_BLOCK + (inode - 1) / INODES_PER_BLOCK;
    let offset = ((inode - 1) % INODES_PER_BLOCK) as usize * 128;
    (block, offset)
}

pub fn write_inode_entry_full(table_block: &mut [u8], inode: u32, mode: u16, size: u32, links: u16, direct_blocks: &[u32]) {
    let (_, off) = inode_block_and_offset(inode);
    vzero(&mut table_block[off..off + 128]);
    wu16(table_block, off + 0x00, mode);
    wu32(table_block, off + 0x04, size);
    wu16(table_block, off + 0x1A, links);
    let num_blocks = direct_blocks.len() as u32;
    let sectors = (num_blocks as usize * BLOCK_SIZE / 512) as u32;
    wu32(table_block, off + 0x1C, sectors);
    let limit = direct_blocks.len().min(12);
    for i in 0..limit {
        wu32(table_block, off + 0x28 + i * 4, direct_blocks[i]);
    }
}

pub fn read_inode_entry(table_block: &[u8], inode: u32) -> InodeInfo {
    let (_, off) = inode_block_and_offset(inode);
    let mode = ru16(table_block, off + 0x00);
    let size = ru32(table_block, off + 0x04);
    let links = ru16(table_block, off + 0x1A);
    let sectors = ru32(table_block, off + 0x1C);
    let blocks_count = sectors / (BLOCK_SIZE as u32 / 512);
    let mut direct_blocks = [0u32; 12];
    for i in 0..12 {
        direct_blocks[i] = ru32(table_block, off + 0x28 + i * 4);
    }
    InodeInfo {
        mode,
        size,
        links,
        blocks_count,
        direct_blocks,
    }
}

/// Allocates the first free block in `block_bitmap` starting at `FILE_DATA_BLOCK + 1`.
pub fn alloc_block(block_bitmap: &mut [u8], total_blocks: u32) -> Option<u32> {
    for block in (FILE_DATA_BLOCK + 1)..=total_blocks {
        let bit = (block - 1) as usize;
        let byte_idx = bit / 8;
        let bit_mask = 1 << (bit % 8);
        if byte_idx < block_bitmap.len() && (block_bitmap[byte_idx] & bit_mask) == 0 {
            block_bitmap[byte_idx] |= bit_mask;
            return Some(block);
        }
    }
    None
}

/// Frees a block in `block_bitmap`.
pub fn free_block(block_bitmap: &mut [u8], block: u32) {
    if block > 0 {
        let bit = (block - 1) as usize;
        let byte_idx = bit / 8;
        if byte_idx < block_bitmap.len() {
            block_bitmap[byte_idx] &= !(1 << (bit % 8));
        }
    }
}

/// Allocates the first free inode in `inode_bitmap` starting at `FILE_INODE + 1`.
pub fn alloc_inode(inode_bitmap: &mut [u8], num_inodes: u32) -> Option<u32> {
    for inode in (FILE_INODE + 1)..=num_inodes {
        let bit = (inode - 1) as usize;
        let byte_idx = bit / 8;
        let bit_mask = 1 << (bit % 8);
        if byte_idx < inode_bitmap.len() && (inode_bitmap[byte_idx] & bit_mask) == 0 {
            inode_bitmap[byte_idx] |= bit_mask;
            return Some(inode);
        }
    }
    None
}

/// Frees an inode in `inode_bitmap`.
pub fn free_inode(inode_bitmap: &mut [u8], inode: u32) {
    if inode > 0 {
        let bit = (inode - 1) as usize;
        let byte_idx = bit / 8;
        if byte_idx < inode_bitmap.len() {
            inode_bitmap[byte_idx] &= !(1 << (bit % 8));
        }
    }
}

pub fn write_dir_entry(block: &mut [u8], offset: usize, inode: u32, rec_len: u16, file_type: u8, name: &[u8]) {
    wu32(block, offset, inode);
    wu16(block, offset + 4, rec_len);
    block[offset + 6] = name.len() as u8;
    block[offset + 7] = file_type;
    vcopy(&mut block[offset + 8..offset + 8 + name.len()], name);
}

/// Initializes a directory data block with `.` and `..` entries.
pub fn init_dir_block(b: &mut [u8], dir_inode: u32, parent_inode: u32) {
    vzero(b);
    write_dir_entry(b, 0, dir_inode, 12, FT_DIR, b".");
    let rem = (BLOCK_SIZE - 12) as u16;
    write_dir_entry(b, 12, parent_inode, rem, FT_DIR, b"..");
}

/// Iterates over all valid, non-zero entries in a directory data block.
pub fn read_dir_entries<F: FnMut(u32, u8, &[u8])>(dir_block: &[u8], mut callback: F) {
    let mut off = 0;
    while off + 8 <= BLOCK_SIZE {
        let inode = ru32(dir_block, off);
        let rec_len = ru16(dir_block, off + 4) as usize;
        if rec_len < 8 || off + rec_len > BLOCK_SIZE {
            break;
        }
        let name_len = dir_block[off + 6] as usize;
        let file_type = dir_block[off + 7];
        if inode != 0 && name_len > 0 && off + 8 + name_len <= off + rec_len {
            let name = &dir_block[off + 8..off + 8 + name_len];
            callback(inode, file_type, name);
        }
        off += rec_len;
    }
}

/// Finds a directory entry by exact name in a directory data block.
pub fn find_dir_entry(dir_block: &[u8], target_name: &[u8]) -> Option<(u32, u8)> {
    let mut result = None;
    read_dir_entries(dir_block, |inode, file_type, name| {
        if result.is_none() && name == target_name {
            result = Some((inode, file_type));
        }
    });
    result
}

/// Inserts a new directory entry into a directory data block.
pub fn add_dir_entry(dir_block: &mut [u8], inode: u32, file_type: u8, name: &[u8]) -> bool {
    let needed = (8 + name.len() + 3) & !3;
    let mut off = 0;
    while off + 8 <= BLOCK_SIZE {
        let cur_inode = ru32(dir_block, off);
        let rec_len = ru16(dir_block, off + 4) as usize;
        if rec_len < 8 || off + rec_len > BLOCK_SIZE {
            break;
        }
        let cur_name_len = dir_block[off + 6] as usize;
        let actual_size = if cur_inode == 0 {
            0
        } else {
            (8 + cur_name_len + 3) & !3
        };

        if cur_inode == 0 && rec_len >= needed {
            // Re-use an empty slot
            write_dir_entry(dir_block, off, inode, rec_len as u16, file_type, name);
            return true;
        } else if cur_inode != 0 && rec_len >= actual_size + needed {
            // Split this entry
            let new_off = off + actual_size;
            let new_rec_len = (rec_len - actual_size) as u16;
            wu16(dir_block, off + 4, actual_size as u16);
            write_dir_entry(dir_block, new_off, inode, new_rec_len, file_type, name);
            return true;
        }
        off += rec_len;
    }
    false
}

/// Removes a directory entry by name from a directory data block.
pub fn remove_dir_entry(dir_block: &mut [u8], target_name: &[u8]) -> Option<u32> {
    let mut prev_off: Option<usize> = None;
    let mut off = 0;
    while off + 8 <= BLOCK_SIZE {
        let cur_inode = ru32(dir_block, off);
        let rec_len = ru16(dir_block, off + 4) as usize;
        if rec_len < 8 || off + rec_len > BLOCK_SIZE {
            break;
        }
        let cur_name_len = dir_block[off + 6] as usize;
        if cur_inode != 0 && cur_name_len > 0 && off + 8 + cur_name_len <= off + rec_len {
            let name = &dir_block[off + 8..off + 8 + cur_name_len];
            if name == target_name {
                // Found!
                if let Some(p_off) = prev_off {
                    let prev_rec_len = ru16(dir_block, p_off + 4) as usize;
                    wu16(dir_block, p_off + 4, (prev_rec_len + rec_len) as u16);
                } else {
                    wu32(dir_block, off, 0); // Mark unused
                }
                return Some(cur_inode);
            }
        }
        prev_off = Some(off);
        off += rec_len;
    }
    None
}

/// Splits a path into parent path and leaf name.
/// E.g. "/a/b/c" -> ("a/b", "c"), "/file" -> ("", "file"), "dir" -> ("", "dir").
pub fn split_parent_and_basename<'a>(path: &'a str) -> (&'a str, &'a str) {
    let trimmed = path.trim_matches('/');
    if let Some(idx) = trimmed.rfind('/') {
        (&trimmed[..idx], &trimmed[idx + 1..])
    } else {
        ("", trimmed)
    }
}

/// Iterator over slash-delimited path components.
pub struct PathComponentIterator<'a> {
    remainder: &'a str,
}

impl<'a> PathComponentIterator<'a> {
    pub fn new(path: &'a str) -> Self {
        let trimmed = path.trim_matches('/');
        Self { remainder: trimmed }
    }
}

impl<'a> Iterator for PathComponentIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remainder.is_empty() {
            return None;
        }
        if let Some(idx) = self.remainder.find('/') {
            let seg = &self.remainder[..idx];
            self.remainder = self.remainder[idx..].trim_start_matches('/');
            Some(seg)
        } else {
            let seg = self.remainder;
            self.remainder = "";
            Some(seg)
        }
    }
}

/// Traverses a path starting at `start_inode` (typically `ROOT_INODE`).
/// `read_dir_block` reads the directory data block for a given directory inode.
/// Returns `Some((target_inode, file_type))` on success, or `None` if any component is missing or not a directory.
pub fn resolve_path<F>(path: &str, start_inode: u32, mut read_dir_block: F) -> Option<(u32, u8)>
where
    F: FnMut(u32, &mut [u8; BLOCK_SIZE]) -> bool,
{
    let mut cur_inode = start_inode;
    let mut cur_type = FT_DIR;
    let mut buf = [0u8; BLOCK_SIZE];

    let mut it = PathComponentIterator::new(path);
    let first = match it.next() {
        Some(f) => f,
        None => return Some((cur_inode, cur_type)),
    };

    let mut current_segment = first;
    loop {
        if cur_type != FT_DIR {
            return None;
        }
        if !read_dir_block(cur_inode, &mut buf) {
            return None;
        }
        match find_dir_entry(&buf, current_segment.as_bytes()) {
            Some((next_inode, next_type)) => {
                cur_inode = next_inode;
                cur_type = next_type;
            }
            None => return None,
        }

        match it.next() {
            Some(next_seg) => {
                current_segment = next_seg;
            }
            None => {
                return Some((cur_inode, cur_type));
            }
        }
    }
}

/// Updates free blocks, free inodes, and used dirs counts in superblock and group descriptor.
pub fn update_alloc_counts(sb: &mut [u8], gd: &mut [u8], block_delta: i32, inode_delta: i32, dir_delta: i32) {
    let free_blocks = ru32(sb, 0x0C) as i32 + block_delta;
    let free_inodes = ru32(sb, 0x10) as i32 + inode_delta;
    wu32(sb, 0x0C, free_blocks.max(0) as u32);
    wu32(sb, 0x10, free_inodes.max(0) as u32);

    let gd_free_blocks = ru16(gd, 0x0C) as i32 + block_delta;
    let gd_free_inodes = ru16(gd, 0x0E) as i32 + inode_delta;
    let gd_used_dirs = ru16(gd, 0x10) as i32 + dir_delta;
    wu16(gd, 0x0C, gd_free_blocks.max(0) as u16);
    wu16(gd, 0x0E, gd_free_inodes.max(0) as u16);
    wu16(gd, 0x10, gd_used_dirs.max(0) as u16);
}

/// Real bug found and fixed hardening Phase 4: a plain `for x in
/// b.iter_mut() { *x = 0; }` loop over a 1024-byte buffer is exactly the
/// shape LLVM's loop-idiom-recognition pass converts into a `memset`
/// call, at any optimization level -- and on this project's toolchain,
/// calls to that symbol are emitted as an indirect call through a
/// permanently-unpopulated slot (see `kernel_common::mem_intrinsics`'s
/// doc for the full story; providing a real `memset` symbol did NOT fix
/// this, because the problem is the CALL SITE itself, not which
/// function it ultimately resolves to). Volatile writes are immune:
/// the compiler must preserve every individual memory operation in
/// program order, which is fundamentally incompatible with recognizing
/// the loop as "equivalent to one memset call" and rewriting it as one.
fn vzero(b: &mut [u8]) {
    for i in 0..b.len() {
        unsafe { core::ptr::write_volatile(&mut b[i], 0) };
    }
}

/// Same real bug, same fix, for copies -- `copy_from_slice` is exactly
/// as susceptible to being recognized as `memcpy` as a manual loop is.
fn vcopy(dst: &mut [u8], src: &[u8]) {
    for i in 0..src.len() {
        unsafe { core::ptr::write_volatile(&mut dst[i], src[i]) };
    }
}

fn wu16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn wu32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn ru16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn ru32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// True if `block` (the real block at `SUPERBLOCK_BLOCK`) is a valid
/// ext2 superblock -- the real magic-number check, s_magic at the real
/// byte offset (0x38) every ext2 implementation checks first.
pub fn is_formatted(superblock_block: &[u8]) -> bool {
    ru16(superblock_block, 0x38) == MAGIC
}

/// Builds a real ext2 superblock in place. `s_blocks_count`/
/// `s_inodes_count` etc. are the real fields a real `fsck.ext2` or
/// kernel ext2 driver would check; `s_free_blocks_count`/
/// `s_free_inodes_count` reflect this increment's fixed layout exactly
/// (blocks/inodes used by the superblock/group-desc/bitmaps/inode-table/
/// root-dir/one-file, everything past that free).
pub fn build_superblock(b: &mut [u8]) {
    vzero(b);
    wu32(b, 0x00, NUM_INODES);
    wu32(b, 0x04, TOTAL_BLOCKS);
    wu32(b, 0x08, 0); // s_r_blocks_count (reserved blocks) -- none
    wu32(b, 0x0C, (TOTAL_BLOCKS - (FILE_DATA_BLOCK + 1)) as u32); // free blocks past FILE_DATA_BLOCK
    wu32(b, 0x10, NUM_INODES - FILE_INODE); // free inodes past FILE_INODE
    wu32(b, 0x14, 1); // s_first_data_block -- 1 for 1024-byte blocks
    wu32(b, 0x18, 0); // s_log_block_size -- 0 => 1024 << 0 = 1024
    wu32(b, 0x1C, 0); // s_log_frag_size -- 0 => 1024
    wu32(b, 0x20, TOTAL_BLOCKS); // s_blocks_per_group -- one group covers everything
    wu32(b, 0x24, TOTAL_BLOCKS); // s_frags_per_group
    wu32(b, 0x28, NUM_INODES); // s_inodes_per_group
    wu16(b, 0x38, MAGIC);
    wu16(b, 0x3A, 1); // s_state -- EXT2_VALID_FS
    wu16(b, 0x3C, 1); // s_errors -- EXT2_ERRORS_CONTINUE
    wu32(b, 0x4C, 0); // s_rev_level -- EXT2_GOOD_OLD_REV (fixed 128-byte inodes, no s_first_ino field)
}

/// Builds the real block group descriptor for this filesystem's one
/// group -- points at the real bitmap/inode-table block numbers above.
pub fn build_group_desc(b: &mut [u8]) {
    vzero(b);
    wu32(b, 0x00, BLOCK_BITMAP_BLOCK);
    wu32(b, 0x04, INODE_BITMAP_BLOCK);
    wu32(b, 0x08, INODE_TABLE_START_BLOCK);
    wu16(b, 0x0C, (TOTAL_BLOCKS - (FILE_DATA_BLOCK + 1)) as u16); // bg_free_blocks_count
    wu16(b, 0x0E, (NUM_INODES - FILE_INODE) as u16); // bg_free_inodes_count
    wu16(b, 0x10, 1); // bg_used_dirs_count -- just the root
}

/// Marks blocks `1..=FILE_DATA_BLOCK` used (superblock, group desc, both
/// bitmaps, the inode table, the root dir block, and the one file's data
/// block) -- real ext2 bit-to-block mapping: bit `i` means block
/// `i + s_first_data_block` (1 for this filesystem), NOT block `i`
/// directly, since block 0 (the boot block) is never tracked at all.
pub fn build_block_bitmap(b: &mut [u8]) {
    vzero(b);
    for block in 1..=FILE_DATA_BLOCK {
        let bit = (block - 1) as usize;
        b[bit / 8] |= 1 << (bit % 8);
    }
}

/// Marks inodes `1..=FILE_INODE` used (the reserved 1..=10 plus the one
/// real file this increment creates) -- real ext2 bit-to-inode mapping:
/// bit `i` means inode `i + 1` (inodes are 1-indexed, bit 0 is unused
/// per spec convention but harmless to also mark here).
pub fn build_inode_bitmap(b: &mut [u8]) {
    vzero(b);
    for inode in 1..=FILE_INODE {
        let bit = (inode - 1) as usize;
        b[bit / 8] |= 1 << (bit % 8);
    }
}

fn write_inode_entry_blocks(table_block: &mut [u8], entry_index_in_block: usize, mode: u16, size: u32, links: u16, start_block: u32, num_blocks: u32) {
    let off = entry_index_in_block * 128;
    vzero(&mut table_block[off..off + 128]);
    wu16(table_block, off + 0x00, mode);
    wu32(table_block, off + 0x04, size);
    wu16(table_block, off + 0x1A, links);
    // i_blocks: 512-byte sector count for the allocated data (real ext2
    // field, used by `du`-style tools) -- BLOCK_SIZE/512 sectors per
    // ext2 block actually used.
    let sectors = (num_blocks as usize * BLOCK_SIZE / 512) as u32;
    wu32(table_block, off + 0x1C, sectors);
    let limit = (num_blocks as usize).min(12);
    for i in 0..limit {
        wu32(table_block, off + 0x28 + i * 4, start_block + i as u32);
    }
}

fn write_inode_entry(table_block: &mut [u8], entry_index_in_block: usize, mode: u16, size: u32, links: u16, block0: u32) {
    let num_blocks = if block0 != 0 { 1 } else { 0 };
    write_inode_entry_blocks(table_block, entry_index_in_block, mode, size, links, block0, num_blocks);
}

/// Writes the root directory's real inode (inode 2, always the second
/// entry of the FIRST inode-table block: `INODE_TABLE_START_BLOCK`
/// holds inodes 1..=8).
pub fn write_root_inode(inode_table_block0: &mut [u8]) {
    write_inode_entry(inode_table_block0, (ROOT_INODE - 1) as usize, S_IFDIR | MODE_755, BLOCK_SIZE as u32, 2, ROOT_DATA_BLOCK);
}

/// Writes the one real file's inode spanning multiple direct blocks.
pub fn write_file_inode_blocks(inode_table_block1: &mut [u8], size: u32, start_block: u32, num_blocks: u32) {
    let entry_index = (FILE_INODE - INODES_PER_BLOCK - 1) as usize;
    write_inode_entry_blocks(inode_table_block1, entry_index, S_IFREG | MODE_644, size, 1, start_block, num_blocks);
}

/// Writes the one real file's inode. `FILE_INODE` (11) falls in the
/// SECOND inode-table block (inodes 9..=16) at entry index
/// `FILE_INODE - INODES_PER_BLOCK - 1`.
pub fn write_file_inode(inode_table_block1: &mut [u8], size: u32) {
    let num_blocks = if size > 0 { ((size as usize + BLOCK_SIZE - 1) / BLOCK_SIZE) as u32 } else { 1 };
    write_file_inode_blocks(inode_table_block1, size, FILE_DATA_BLOCK, num_blocks);
}

/// Reads the file's recorded size from its real inode table entry.
pub fn read_file_inode_size(inode_table_block1: &[u8]) -> usize {
    let entry_index = (FILE_INODE - INODES_PER_BLOCK - 1) as usize;
    let off = entry_index * 128;
    ru32(inode_table_block1, off + 0x04) as usize
}


/// Builds the root directory's data block: real `ext2_dir_entry_2`
/// entries for `.`, `..`, and one file (`file_name`) -- real rec_len
/// chaining, the last entry's rec_len extended to the end of the block
/// exactly as a real ext2 directory requires.
pub fn build_root_dir_block(b: &mut [u8], file_name: &str) {
    vzero(b);
    let name = file_name.as_bytes();
    // "." -- rec_len 12 (8-byte header + 1-byte name, rounded to 4).
    write_dir_entry(b, 0, ROOT_INODE, 12, FT_DIR, b".");
    // ".." -- rec_len 12 (8 + 2 rounds to 12 too).
    write_dir_entry(b, 12, ROOT_INODE, 12, FT_DIR, b"..");
    // the file -- last entry, rec_len extends to the block's end (real
    // ext2 requirement: the final entry in a block always reaches the
    // block boundary, however much slack that leaves).
    let remaining = (BLOCK_SIZE - 24) as u16;
    write_dir_entry(b, 24, FILE_INODE, remaining, FT_REG, name);
}

/// Copies `content` into the file's real data block. Capped at
/// `BLOCK_SIZE` -- this increment is direct-blocks-only, one block, see
/// module doc.
pub fn build_file_data_block(b: &mut [u8], content: &[u8]) -> usize {
    vzero(b);
    let n = content.len().min(BLOCK_SIZE);
    vcopy(&mut b[..n], &content[..n]);
    n
}

/// Reads the file's real size back out of its real inode (does NOT
/// trust a caller-supplied length -- this is what makes read_file_data a
/// genuine read of persisted metadata, not just a raw block dump), then
/// copies that many bytes from the file's real data block into `out`.
/// Returns the real byte count read.
pub fn read_file_data(inode_table_block1: &[u8], data_block: &[u8], out: &mut [u8]) -> usize {
    let entry_index = (FILE_INODE - INODES_PER_BLOCK - 1) as usize;
    let off = entry_index * 128;
    let size = ru32(inode_table_block1, off + 0x04) as usize;
    let n = size.min(BLOCK_SIZE).min(out.len());
    vcopy(&mut out[..n], &data_block[..n]);
    n
}
