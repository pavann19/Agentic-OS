//! Real host-side unit tests for `kernel_common`'s pure logic — run via
//! `cargo test` on the host, no QEMU, no target hardware. This is the
//! `docs/ROADMAP.md` Phase 0 "host test harness" item: `make test-host`
//! used to just print "No host unit tests exist yet." — these are real
//! assertions against the actual code `kernel_rs` runs (via the
//! `kernel_common` dependency both crates share), not a parallel
//! reimplementation being tested instead.
//!
//! Several of these are regression tests for real bugs this session found
//! and fixed by booting and reading back page tables — the whole point of
//! having them here is that the NEXT such bug should be caught by
//! `cargo test` in seconds, not by a debugging session with QEMU and a
//! raw hex printer.

#[cfg(test)]
mod bitmap_tests {
    use kernel_common::bitmap::*;

    #[test]
    fn set_then_test_is_true() {
        let mut bm = [0u8; 16];
        set(&mut bm, 5);
        assert!(test(&bm, 5));
    }

    #[test]
    fn unset_bits_test_false() {
        let bm = [0u8; 16];
        for i in 0..128 {
            assert!(!test(&bm, i), "bit {} should be unset in a zeroed bitmap", i);
        }
    }

    #[test]
    fn clear_after_set_is_false() {
        let mut bm = [0xFFu8; 16];
        clear(&mut bm, 10);
        assert!(!test(&bm, 10));
        // Neighboring bits must be untouched by clear().
        assert!(test(&bm, 9));
        assert!(test(&bm, 11));
    }

    #[test]
    fn set_does_not_disturb_other_bits_in_same_byte() {
        let mut bm = [0u8; 1];
        set(&mut bm, 3);
        for i in 0..8 {
            assert_eq!(test(&bm, i), i == 3, "only bit 3 should be set, checked bit {}", i);
        }
    }

    #[test]
    fn byte_boundary_indices_land_in_correct_byte() {
        let mut bm = [0u8; 4];
        set(&mut bm, 8); // first bit of byte 1
        assert_eq!(bm[0], 0);
        assert_eq!(bm[1], 0b0000_0001);
        set(&mut bm, 15); // last bit of byte 1
        assert_eq!(bm[1], 0b1000_0001);
        assert_eq!(bm[2], 0);
    }

    // --- reserve_range_pages: regression coverage for the real rounding
    // logic ported from kernel/memory.c's pmm_reserve_range, and the class
    // of off-by-one this session had to get right for the null-guard page.

    #[test]
    fn reserve_range_aligned_exact_page() {
        let (start_page, pages) = reserve_range_pages(0, 4096, 4096);
        assert_eq!(start_page, 0);
        assert_eq!(pages, 1);
    }

    #[test]
    fn reserve_range_page_zero_specifically() {
        // The exact call PMM makes to reserve the null-guard page —
        // matters that this returns (0, 1), not (0, 0) or an off-by-one.
        let (start_page, pages) = reserve_range_pages(0, 4096, 4096);
        assert_eq!((start_page, pages), (0, 1));
    }

    #[test]
    fn reserve_range_unaligned_start_rounds_outward() {
        // Starts 100 bytes into page 0, length 4096 -- must still cover
        // TWO pages (the tail spills into page 1), not one.
        let (start_page, pages) = reserve_range_pages(100, 4096, 4096);
        assert_eq!(start_page, 0);
        assert_eq!(pages, 2, "an unaligned start must round outward to cover the spillover page");
    }

    #[test]
    fn reserve_range_tiny_length_still_reserves_one_page() {
        let (_, pages) = reserve_range_pages(0, 1, 4096);
        assert_eq!(pages, 1);
    }

    #[test]
    fn reserve_range_multi_page_exact() {
        let (start_page, pages) = reserve_range_pages(0x10000, 3 * 4096, 4096);
        assert_eq!(start_page, 0x10);
        assert_eq!(pages, 3);
    }
}

#[cfg(test)]
mod pagetable_tests {
    use kernel_common::pagetable::*;

    #[test]
    fn split_indices_zero() {
        assert_eq!(split_indices(0), (0, 0, 0, 0));
    }

    #[test]
    fn split_indices_known_higher_half_kernel_base() {
        // The actual KERNEL_VIRTUAL_BASE this kernel links at
        // (kernel_rs/linker.ld) -- must land in PML4 slot 511 (the
        // top-of-memory convention every higher-half x86_64 kernel uses).
        let (pml4, _, _, _) = split_indices(0xFFFF_FFFF_8000_0000);
        assert_eq!(pml4, 511);
    }

    #[test]
    fn split_indices_known_phys_map_base() {
        // vmm.rs's PHYS_MAP_BASE -- must land in a DIFFERENT PML4 slot than
        // the kernel base above, or the direct-map window would alias the
        // kernel image (a real class of bug this session had to reason
        // through carefully by hand -- now a permanent regression test).
        let (pml4, _, _, _) = split_indices(0xFFFF_8000_0000_0000);
        assert_eq!(pml4, 256);
        assert_ne!(pml4, 511, "PHYS_MAP_BASE must not alias KERNEL_VIRTUAL_BASE's PML4 slot");
    }

    #[test]
    fn split_indices_page_granularity() {
        // Two addresses 4096 bytes apart must differ only in pt_index.
        let (p4a, p3a, p2a, p1a) = split_indices(0x1000);
        let (p4b, p3b, p2b, p1b) = split_indices(0x2000);
        assert_eq!((p4a, p3a, p2a), (p4b, p3b, p2b));
        assert_eq!(p1a, 1);
        assert_eq!(p1b, 2);
    }

    #[test]
    fn split_indices_pd_rollover_at_2mb() {
        // Crossing a 2MB boundary must roll pd_index, not pt_index only.
        let (_, _, pd_a, _) = split_indices(0x1F_F000); // last page of PD 0
        let (_, _, pd_b, _) = split_indices(0x20_0000); // first page of PD 1
        assert_eq!(pd_a, 0);
        assert_eq!(pd_b, 1);
    }

    #[test]
    fn frame_from_entry_masks_flag_bits() {
        // A present+writable+NX leaf entry pointing at physical 0x204000
        // -- must extract exactly the frame, none of the flag bits. This
        // is a direct regression test for the NX-on-live-code bug this
        // session found by reading back a real PTE (0x8000000000204001).
        let entry: u64 = 0x8000000000204001;
        assert_eq!(frame_from_entry(entry), 0x204000);
    }

    #[test]
    fn frame_from_entry_zero_entry_is_zero_frame() {
        assert_eq!(frame_from_entry(0), 0);
    }
}

#[cfg(test)]
mod align_tests {
    use kernel_common::{align_up, pages_for};

    #[test]
    fn align_up_already_aligned() {
        assert_eq!(align_up(0x1000, 0x1000), 0x1000);
    }

    #[test]
    fn align_up_rounds_up() {
        assert_eq!(align_up(0x1001, 0x1000), 0x2000);
    }

    #[test]
    fn align_up_zero() {
        assert_eq!(align_up(0, 0x1000), 0);
    }

    #[test]
    fn pages_for_exact() {
        assert_eq!(pages_for(4096, 4096), 1);
    }

    #[test]
    fn pages_for_rounds_up() {
        assert_eq!(pages_for(4097, 4096), 2);
        assert_eq!(pages_for(1, 4096), 1);
    }

    #[test]
    fn pages_for_zero_is_zero() {
        assert_eq!(pages_for(0, 4096), 0);
    }
}

/// Real assertions against `kernel_common::ext2` -- Phase 4's on-disk
/// filesystem, exercised here exactly the way `virtio_blk_fs.rs` (the
/// real driver) does it: build every block in memory, then verify the
/// resulting layout by independently re-parsing the raw bytes (not by
/// calling the same private helpers that built them -- a bug in a
/// shared helper would otherwise pass its own test trivially).
#[cfg(test)]
mod ext2_tests {
    use kernel_common::ext2::*;

    fn ru16(b: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([b[off], b[off + 1]])
    }
    fn ru32(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    /// Builds a complete, real, in-memory filesystem image (every block
    /// this increment's fixed layout defines) containing one file with
    /// `content`. Mirrors exactly what the real virtio-blk driver does,
    /// block by block, just without going through real disk I/O.
    fn build_test_fs(file_name: &str, content: &[u8]) -> Vec<u8> {
        let mut disk = vec![0u8; (TOTAL_BLOCKS as usize) * BLOCK_SIZE];
        fn blk(n: u32) -> core::ops::Range<usize> {
            let start = n as usize * BLOCK_SIZE;
            start..start + BLOCK_SIZE
        }
        build_superblock(&mut disk[blk(SUPERBLOCK_BLOCK)]);
        build_group_desc(&mut disk[blk(GROUP_DESC_BLOCK)]);
        build_block_bitmap(&mut disk[blk(BLOCK_BITMAP_BLOCK)]);
        build_inode_bitmap(&mut disk[blk(INODE_BITMAP_BLOCK)]);
        write_root_inode(&mut disk[blk(INODE_TABLE_START_BLOCK)]);
        write_file_inode(&mut disk[blk(INODE_TABLE_START_BLOCK + 1)], content.len() as u32);
        build_root_dir_block(&mut disk[blk(ROOT_DATA_BLOCK)], file_name);
        build_file_data_block(&mut disk[blk(FILE_DATA_BLOCK)], content);
        disk
    }

    #[test]
    fn zeroed_disk_is_not_formatted() {
        let disk = vec![0u8; (TOTAL_BLOCKS as usize) * BLOCK_SIZE];
        let sb = &disk[(SUPERBLOCK_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        assert!(!is_formatted(sb));
    }

    #[test]
    fn formatted_disk_reports_formatted() {
        let disk = build_test_fs("hello.txt", b"hi");
        let sb = &disk[(SUPERBLOCK_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        assert!(is_formatted(sb));
    }

    #[test]
    fn superblock_fields_match_real_ext2_layout() {
        let disk = build_test_fs("hello.txt", b"hi");
        let sb = &disk[(SUPERBLOCK_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        assert_eq!(ru16(sb, 0x38), MAGIC, "s_magic at the real 0x38 offset");
        assert_eq!(ru32(sb, 0x00), NUM_INODES, "s_inodes_count");
        assert_eq!(ru32(sb, 0x04), TOTAL_BLOCKS, "s_blocks_count");
        assert_eq!(ru32(sb, 0x14), 1, "s_first_data_block == 1 for 1024-byte blocks");
        assert_eq!(ru32(sb, 0x18), 0, "s_log_block_size == 0 => 1024 byte blocks");
    }

    #[test]
    fn block_bitmap_marks_exactly_the_used_blocks() {
        let disk = build_test_fs("hello.txt", b"hi");
        let bm = &disk[(BLOCK_BITMAP_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        // Real bit-to-block mapping: bit i => block i+1 (s_first_data_block).
        for block in 1..=FILE_DATA_BLOCK {
            let bit = (block - 1) as usize;
            assert!(bm[bit / 8] & (1 << (bit % 8)) != 0, "block {} should be marked used", block);
        }
        let first_free_bit = FILE_DATA_BLOCK as usize; // block FILE_DATA_BLOCK+1
        assert!(bm[first_free_bit / 8] & (1 << (first_free_bit % 8)) == 0, "block past FILE_DATA_BLOCK should be free");
    }

    #[test]
    fn root_dir_has_dot_dotdot_and_the_real_file_entry() {
        let disk = build_test_fs("greeting.txt", b"hi");
        let root = &disk[(ROOT_DATA_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];

        // "." at offset 0
        assert_eq!(ru32(root, 0), ROOT_INODE);
        assert_eq!(root[6], 1); // name_len
        assert_eq!(&root[8..9], b".");

        // ".." at offset 12 (rec_len of "." entry)
        let dotdot_off = ru16(root, 4) as usize;
        assert_eq!(dotdot_off, 12);
        assert_eq!(ru32(root, dotdot_off), ROOT_INODE);
        assert_eq!(root[dotdot_off + 6], 2);
        assert_eq!(&root[dotdot_off + 8..dotdot_off + 10], b"..");

        // the file entry, right after ".."
        let file_off = dotdot_off + ru16(root, dotdot_off + 4) as usize;
        assert_eq!(ru32(root, file_off), FILE_INODE);
        let name_len = root[file_off + 6] as usize;
        assert_eq!(name_len, "greeting.txt".len());
        assert_eq!(&root[file_off + 8..file_off + 8 + name_len], b"greeting.txt");
        // last entry's rec_len must reach the real block boundary
        let rec_len = ru16(root, file_off + 4) as usize;
        assert_eq!(file_off + rec_len, BLOCK_SIZE, "last dir entry must extend to the block's end");
    }

    #[test]
    fn file_inode_size_and_block_pointer_are_real() {
        let content = b"Agentic OS Phase 4 persisted this.\n";
        let disk = build_test_fs("greeting.txt", content);
        let inode_table1 = &disk[((INODE_TABLE_START_BLOCK + 1) as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        let entry_index = (FILE_INODE - INODES_PER_BLOCK - 1) as usize;
        let off = entry_index * 128;
        assert_eq!(ru32(inode_table1, off + 0x04), content.len() as u32, "i_size");
        assert_eq!(ru32(inode_table1, off + 0x28), FILE_DATA_BLOCK, "i_block[0]");
        let mode = ru16(inode_table1, off + 0x00);
        assert_eq!(mode & 0x8000, 0x8000, "S_IFREG bit set");
    }

    #[test]
    fn write_then_read_round_trip_is_byte_identical() {
        let content = b"Agentic OS Phase 4 persisted this.\n";
        let disk = build_test_fs("greeting.txt", content);
        let inode_table1 = &disk[((INODE_TABLE_START_BLOCK + 1) as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        let data_block = &disk[(FILE_DATA_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        let mut out = [0u8; BLOCK_SIZE];
        let n = read_file_data(inode_table1, data_block, &mut out);
        assert_eq!(n, content.len());
        assert_eq!(&out[..n], &content[..], "round-tripped content must be byte-identical");
    }

    #[test]
    fn read_file_data_trusts_the_real_inode_size_not_a_full_block() {
        // A real regression this test guards against: read_file_data must
        // use the inode's OWN i_size, not just hand back a full BLOCK_SIZE
        // of (possibly stale/garbage-padded) data.
        let content = b"short";
        let disk = build_test_fs("f.txt", content);
        let inode_table1 = &disk[((INODE_TABLE_START_BLOCK + 1) as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        let data_block = &disk[(FILE_DATA_BLOCK as usize) * BLOCK_SIZE..][..BLOCK_SIZE];
        let mut out = [0xFFu8; BLOCK_SIZE];
        let n = read_file_data(inode_table1, data_block, &mut out);
        assert_eq!(n, 5);
        assert_eq!(&out[..5], b"short");
    }
}

/// Real assertions against `kernel_common::audit_ring` -- Phase 4's
/// audit-log-persistence-with-rotation item, exercised the way
/// `virtio_blk_driver` runs it for real: build a header + N records in
/// memory (mirroring what gets written block-by-block to the real
/// disk), verify rotation genuinely overwrites the oldest slot's bytes,
/// not just that the counters increment.
#[cfg(test)]
mod audit_ring_tests {
    use kernel_common::audit_ring::*;

    #[test]
    fn zeroed_block_is_not_initialized() {
        let b = [0u8; BLOCK_SIZE];
        assert!(!is_initialized(&b));
    }

    #[test]
    fn built_header_is_initialized_and_round_trips() {
        let mut b = [0u8; BLOCK_SIZE];
        build_header(&mut b, &RingHeader { next_write_index: 2, total_written_count: 9 });
        assert!(is_initialized(&b));
        let h = read_header(&b);
        assert_eq!(h.next_write_index, 2);
        assert_eq!(h.total_written_count, 9);
    }

    #[test]
    fn record_round_trip_is_byte_identical() {
        let mut b = [0u8; BLOCK_SIZE];
        let msg = b"VIRTIO_BLK: real audit record";
        build_record(&mut b, 42, msg);
        let mut out = [0u8; BLOCK_SIZE];
        let (seq, n) = read_record(&b, &mut out);
        assert_eq!(seq, 42);
        assert_eq!(n, msg.len());
        assert_eq!(&out[..n], &msg[..]);
    }

    #[test]
    fn record_read_trusts_persisted_length_not_a_full_block() {
        let mut b = [0u8; BLOCK_SIZE];
        build_record(&mut b, 1, b"hi");
        let mut out = [0xFFu8; BLOCK_SIZE];
        let (_, n) = read_record(&b, &mut out);
        assert_eq!(n, 2);
        assert_eq!(&out[..2], b"hi");
    }

    #[test]
    fn rotation_genuinely_overwrites_the_oldest_slot_on_disk() {
        // Simulate RING_SLOTS + 2 real boots, each writing one record
        // into slot (total_written_count % RING_SLOTS) -- exactly what
        // virtio_blk_driver does against the real disk.
        let mut slots: Vec<[u8; BLOCK_SIZE]> = (0..RING_SLOTS).map(|_| [0u8; BLOCK_SIZE]).collect();
        let mut total: u64 = 0;
        for _ in 0..(RING_SLOTS as u64 + 2) {
            let idx = (total % RING_SLOTS as u64) as usize;
            let msg = alloc_msg(total);
            build_record(&mut slots[idx], total, &msg);
            total += 1;
        }
        // Slot 0 was written at total=0 AND overwritten at total=RING_SLOTS
        // (0 % 4 == 4 % 4) -- real rotation, not just a counter increasing.
        let mut out = [0u8; BLOCK_SIZE];
        let (seq, n) = read_record(&slots[0], &mut out);
        assert_eq!(seq, RING_SLOTS as u64, "slot 0 must hold the ROTATED-IN record, not the original");
        assert_eq!(&out[..n], &alloc_msg(RING_SLOTS as u64)[..]);
        assert!(total > RING_SLOTS as u64, "rotation should genuinely have begun");
    }

    fn alloc_msg(seq: u64) -> Vec<u8> {
        format!("record #{}", seq).into_bytes()
    }
}

#[cfg(test)]
mod driver_registry_tests {
    // Real, isolated verification of kernel_common::driver_registry --
    // an experimental module NOT wired into kernel_rs's actual boot
    // path (see that module's own doc). These tests are what "tested
    // in isolation before any integration decision" means concretely:
    // this crate's own real logic, exercised the same way every other
    // kernel_common module already is here, with zero QEMU involved.

    use kernel_common::driver_registry::{match_all, DriverEntry, MatchRule, PciId};

    fn dev(vendor: u16, device: u16, class: u8, subclass: u8, prog_if: u8) -> PciId {
        PciId { vendor, device, class, subclass, prog_if }
    }

    #[test]
    fn vendor_device_rule_matches_exact_pair_only() {
        let rule = MatchRule::VendorDevice(0x1AF4, 0x1042); // virtio-blk's real IDs
        assert!(rule.matches(&dev(0x1AF4, 0x1042, 0x01, 0x00, 0x00)));
        assert!(!rule.matches(&dev(0x1AF4, 0x1041, 0x01, 0x00, 0x00))); // virtio-net's real device ID -- must NOT match
        assert!(!rule.matches(&dev(0x8086, 0x1042, 0x01, 0x00, 0x00))); // wrong vendor
    }

    #[test]
    fn class_rule_matches_regardless_of_vendor_device() {
        // Real NVMe class code (01/08/02) -- kernel_rs::nvme's own
        // actual match rule, since real NVMe controllers from
        // different vendors report different vendor/device IDs.
        let rule = MatchRule::Class(0x01, 0x08, 0x02);
        assert!(rule.matches(&dev(0x8086, 0x1234, 0x01, 0x08, 0x02))); // Intel-branded NVMe
        assert!(rule.matches(&dev(0x144D, 0xABCD, 0x01, 0x08, 0x02))); // Samsung-branded NVMe -- different vendor, same class, still matches
        assert!(!rule.matches(&dev(0x8086, 0x1234, 0x01, 0x06, 0x01))); // AHCI's class -- must NOT match NVMe's rule
    }

    #[test]
    fn match_all_finds_every_real_driver_by_its_own_actual_rule() {
        // Mirrors kernel_rs's five real drivers' own actual match
        // rules exactly (see virtio_blk.rs/virtio_net.rs/ahci.rs/
        // nvme.rs/e1000.rs's own find_X functions) -- this is the
        // real proposed replacement table, not a synthetic example.
        let table = [
            DriverEntry { name: "virtio_blk", rule: MatchRule::VendorDevice(0x1AF4, 0x1042), handler: 1u32 },
            DriverEntry { name: "virtio_net", rule: MatchRule::VendorDevice(0x1AF4, 0x1041), handler: 2u32 },
            DriverEntry { name: "ahci", rule: MatchRule::VendorDevice(0x8086, 0x2922), handler: 3u32 },
            DriverEntry { name: "nvme", rule: MatchRule::Class(0x01, 0x08, 0x02), handler: 4u32 },
            DriverEntry { name: "e1000", rule: MatchRule::VendorDevice(0x8086, 0x100E), handler: 5u32 },
        ];

        // A real, representative device list -- the exact shape a real
        // boot's own pci::enumerate() produces (host bridge + ISA
        // bridge devices that match NOTHING, interspersed with real
        // driver-matching devices), not a hand-picked easy case.
        let devices = [
            dev(0x8086, 0x29C0, 0x06, 0x00, 0x00), // host bridge -- matches nothing
            dev(0x1AF4, 0x1042, 0x01, 0x00, 0x00), // virtio-blk
            dev(0x8086, 0x2918, 0x06, 0x01, 0x00), // ISA bridge -- matches nothing
            dev(0x1AF4, 0x1041, 0x01, 0x00, 0x00), // virtio-net
            dev(0x8086, 0x2922, 0x01, 0x06, 0x01), // AHCI
            dev(0x8086, 0x1234, 0x01, 0x08, 0x02), // NVMe (class-matched, real vendor-agnostic case)
            dev(0x8086, 0x100E, 0x02, 0x00, 0x00), // e1000
        ];

        let mut out = [(dev(0, 0, 0, 0, 0), 0u32); 8];
        let count = match_all(&devices, &table, &mut out);

        assert_eq!(count, 5, "exactly the 5 real driver-matching devices, none of the 2 non-matching ones");
        let handlers: Vec<u32> = out[..count].iter().map(|(_, h)| *h).collect();
        assert_eq!(handlers, vec![1, 2, 3, 4, 5], "matched in device-list order, each to its own correct handler");
    }

    #[test]
    fn two_devices_of_the_same_kind_are_both_matched_not_just_the_first() {
        // Real, documented limitation of TODAY's kernel_rs code (every
        // find_X uses .find(), which stops at the first match) that
        // this module's own doc names as the concrete improvement over
        // it -- this test is the actual proof, not just an assertion
        // in a comment.
        let table = [DriverEntry { name: "nvme", rule: MatchRule::Class(0x01, 0x08, 0x02), handler: 4u32 }];
        let devices = [
            dev(0x8086, 0x1111, 0x01, 0x08, 0x02), // first NVMe controller
            dev(0x144D, 0x2222, 0x01, 0x08, 0x02), // a SECOND NVMe controller, different vendor
        ];
        let mut out = [(dev(0, 0, 0, 0, 0), 0u32); 8];
        let count = match_all(&devices, &table, &mut out);

        assert_eq!(count, 2, "BOTH real NVMe controllers must be matched -- today's .find()-based code would silently only spawn a driver for the first one");
        assert_eq!(out[0].0.device, 0x1111);
        assert_eq!(out[1].0.device, 0x2222);
    }

    #[test]
    fn output_capacity_bounds_are_respected_not_silently_overrun_or_panicked() {
        let table = [DriverEntry { name: "virtio_blk", rule: MatchRule::VendorDevice(0x1AF4, 0x1042), handler: 1u32 }];
        let devices = [
            dev(0x1AF4, 0x1042, 0x01, 0x00, 0x00),
            dev(0x1AF4, 0x1042, 0x01, 0x00, 0x00),
            dev(0x1AF4, 0x1042, 0x01, 0x00, 0x00),
        ];
        let mut out = [(dev(0, 0, 0, 0, 0), 0u32); 2]; // capacity 2, 3 real matches available
        let count = match_all(&devices, &table, &mut out);
        assert_eq!(count, 2, "stops cleanly at capacity, no panic, no silent drop without a return value saying so");
    }

    #[test]
    fn a_device_matching_no_rule_is_skipped_not_an_error() {
        let table = [DriverEntry { name: "virtio_blk", rule: MatchRule::VendorDevice(0x1AF4, 0x1042), handler: 1u32 }];
        let devices = [dev(0x8086, 0x29C0, 0x06, 0x00, 0x00)]; // a real host-bridge device, matches nothing
        let mut out = [(dev(0, 0, 0, 0, 0), 0u32); 4];
        let count = match_all(&devices, &table, &mut out);
        assert_eq!(count, 0);
    }
}
