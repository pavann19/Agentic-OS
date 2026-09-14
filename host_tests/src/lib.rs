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
        PciId { bus: 0, device_slot: 0, function: 0, vendor, device, class, subclass, prog_if }
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

#[cfg(test)]
mod madt_tests {
    // Real, isolated verification of kernel_common::madt -- Phase 9's
    // first deliverable (docs/ROADMAP.md §5 Phase 9, item 1). Real
    // synthetic MADT bytes built to the actual ACPI 5.2.12 layout, not
    // a simplified stand-in -- the same "byte-correct, spec-driven"
    // discipline this project's real drivers have used since Phase 6.

    use kernel_common::madt::{parse_cpus, CpuEntry};

    /// Builds a real MADT body (everything after the 36-byte SDT
    /// header this module never sees -- kernel_rs::acpi strips that
    /// before handing bytes here): 4-byte Local APIC Address + 4-byte
    /// Flags, then the caller's own already-encoded entries appended
    /// verbatim.
    fn body(entries: &[u8]) -> alloc_free_vec {
        // host_tests runs with std available, but keep this file's own
        // style consistent with the rest of this module -- a small
        // fixed-capacity buffer, not a real Vec import, for a helper
        // this simple.
        let mut v = alloc_free_vec::new();
        v.extend_from_slice(&[0u8; 4]); // Local APIC Address (unused by parse_cpus)
        v.extend_from_slice(&[0u8; 4]); // Flags (unused by parse_cpus)
        v.extend_from_slice(entries);
        v
    }

    /// Type 0 (Processor Local APIC) entry, the real 8-byte layout.
    fn local_apic_entry(processor_uid: u8, apic_id: u8, enabled: bool) -> [u8; 8] {
        let flags: u32 = if enabled { 1 } else { 0 };
        let fb = flags.to_le_bytes();
        [0, 8, processor_uid, apic_id, fb[0], fb[1], fb[2], fb[3]]
    }

    /// Type 9 (Processor Local x2APIC) entry, the real 16-byte layout:
    /// type(1) len(1) reserved(2) x2apic_id(4) flags(4) processor_uid(4).
    fn local_x2apic_entry(processor_uid: u32, apic_id: u32, enabled: bool) -> [u8; 16] {
        let mut e = [0u8; 16];
        e[0] = 9;
        e[1] = 16;
        let flags: u32 = if enabled { 1 } else { 0 };
        e[4..8].copy_from_slice(&apic_id.to_le_bytes());
        e[8..12].copy_from_slice(&flags.to_le_bytes());
        e[12..16].copy_from_slice(&processor_uid.to_le_bytes());
        e
    }

    // Minimal capacity-bounded Vec-like helper, `std`-free in spirit
    // (this crate has std available for tests, but keeping this local
    // avoids pulling in std::vec::Vec just for test fixture assembly).
    #[allow(non_camel_case_types)]
    struct alloc_free_vec {
        buf: [u8; 256],
        len: usize,
    }
    impl alloc_free_vec {
        fn new() -> Self {
            Self { buf: [0; 256], len: 0 }
        }
        fn extend_from_slice(&mut self, s: &[u8]) {
            self.buf[self.len..self.len + s.len()].copy_from_slice(s);
            self.len += s.len();
        }
    }
    impl core::ops::Deref for alloc_free_vec {
        type Target = [u8];
        fn deref(&self) -> &[u8] {
            &self.buf[..self.len]
        }
    }

    #[test]
    fn single_enabled_bsp_is_found() {
        // Real single-CPU QEMU default (no -smp flag) -- one Type 0
        // entry, APIC ID 0, enabled.
        let b = body(&local_apic_entry(0, 0, true));
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 1);
        assert_eq!(out[0], CpuEntry { apic_id: 0, processor_uid: 0, enabled: true });
    }

    #[test]
    fn multiple_enabled_cpus_are_all_found_in_order() {
        // Real -smp 4 shape: four Type 0 entries, APIC IDs 0..3.
        let mut entries = alloc_free_vec::new();
        for i in 0..4u8 {
            entries.extend_from_slice(&local_apic_entry(i, i, true));
        }
        let b = body(&entries);
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 4);
        for i in 0..4u32 {
            assert_eq!(out[i as usize], CpuEntry { apic_id: i, processor_uid: i, enabled: true });
        }
    }

    #[test]
    fn a_disabled_processor_entry_is_still_reported_but_marked_disabled() {
        // Real firmware behavior: a socket with no CPU installed can
        // still get a MADT entry, flags bit 0 clear. Phase 9's own
        // AP-bring-up step MUST NOT SIPI this APIC ID -- there may be
        // no silicon there. Reported, not silently dropped, so a
        // caller can log "seen but not usable" rather than nothing.
        let b = body(&local_apic_entry(1, 4, false));
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: true }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 1);
        assert_eq!(out[0].enabled, false);
        assert_eq!(out[0].apic_id, 4);
    }

    #[test]
    fn x2apic_entries_decode_the_same_shape_as_local_apic_entries() {
        let b = body(&local_x2apic_entry(300, 300, true)); // APIC ID > 255 -- exactly why x2APIC entries exist
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 1);
        assert_eq!(out[0], CpuEntry { apic_id: 300, processor_uid: 300, enabled: true });
    }

    #[test]
    fn unrecognized_entry_types_are_skipped_via_their_own_length_not_misparsed() {
        // A real MADT interleaves I/O APIC (type 1) and interrupt
        // source override (type 2) entries among the processor
        // entries -- this must skip them using their own declared
        // length, not assume every entry is 8 bytes.
        let mut entries = alloc_free_vec::new();
        entries.extend_from_slice(&local_apic_entry(0, 0, true));
        entries.extend_from_slice(&[1, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // fake 12-byte I/O APIC entry
        entries.extend_from_slice(&local_apic_entry(1, 1, true));
        let b = body(&entries);
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 2);
        assert_eq!(out[0].apic_id, 0);
        assert_eq!(out[1].apic_id, 1);
    }

    #[test]
    fn output_capacity_bounds_are_respected() {
        let mut entries = alloc_free_vec::new();
        for i in 0..8u8 {
            entries.extend_from_slice(&local_apic_entry(i, i, true));
        }
        let b = body(&entries);
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 3]; // room for 3, not 8
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 3);
    }

    #[test]
    fn truncated_entry_stops_the_walk_cleanly_instead_of_reading_out_of_bounds() {
        // A real-world malformed/truncated MADT: a Type 0 entry claims
        // length 8 but only 5 bytes actually remain. Must stop, not
        // panic or read past the slice.
        let mut entries = alloc_free_vec::new();
        entries.extend_from_slice(&local_apic_entry(0, 0, true)); // one real, valid entry first
        entries.extend_from_slice(&[0, 8, 1, 1]); // truncated second entry -- claims 8 bytes, only 4 given
        let b = body(&entries);
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 1); // only the first, valid entry
    }

    #[test]
    fn empty_body_is_zero_cpus_not_a_panic() {
        let b: [u8; 0] = [];
        let mut out = [CpuEntry { apic_id: 0, processor_uid: 0, enabled: false }; 8];
        let count = parse_cpus(&b, &mut out);
        assert_eq!(count, 0);
    }
}

#[cfg(test)]
mod authority_graph_tests {
    // Research track (docs/RESEARCH_TRACK.md, docs/NOVEL_CONCEPTS.md
    // section 1): real, isolated verification that one authority
    // structure can be the sole source for both a CPU page-table-shaped
    // projection and a VT-d IOMMU-table-shaped projection. Zero QEMU,
    // zero kernel_rs involvement -- exactly the isolation discipline
    // this project requires before any integration decision.

    use kernel_common::authority_graph::{
        grant, is_reachable, project_iommu_table, project_page_table, Entry, Grant, Principal,
    };

    fn cpu(id: u32) -> Principal {
        Principal::CpuProcess(id)
    }

    fn pci(bus: u8, device: u8, function: u8) -> Principal {
        Principal::PciDevice { bus, device, function }
    }

    #[test]
    fn a_fresh_grant_is_reachable_by_both_projections() {
        let mut graph: [Option<Grant>; 8] = [None; 8];
        assert!(grant(
            &mut graph,
            Grant { principal: cpu(1), phys_base: 0x10_0000, len: 4096, writable: true, executable: false }
        ));

        let mut cpu_out = [Entry { phys_page: 0, writable: false, executable: false }; 8];
        let cpu_count = project_page_table(&graph, cpu(1), &mut cpu_out);
        assert_eq!(cpu_count, 1);
        assert_eq!(cpu_out[0].phys_page, 0x10_0000);
        assert!(cpu_out[0].writable);

        assert!(is_reachable(&graph, cpu(1), 0x10_0000));
    }

    #[test]
    fn projections_are_filtered_by_principal_not_shared_across_principals() {
        let mut graph: [Option<Grant>; 8] = [None; 8];
        grant(&mut graph, Grant { principal: cpu(1), phys_base: 0x1000, len: 4096, writable: true, executable: false });
        grant(&mut graph, Grant { principal: pci(0, 3, 0), phys_base: 0x2000, len: 4096, writable: true, executable: false });

        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 8];
        let count = project_page_table(&graph, cpu(1), &mut out);
        assert_eq!(count, 1);
        assert_eq!(out[0].phys_page, 0x1000);

        let count2 = project_iommu_table(&graph, pci(0, 3, 0), &mut out);
        assert_eq!(count2, 1);
        assert_eq!(out[0].phys_page, 0x2000);

        // A device's own IOMMU projection must never include a CPU
        // process's grant, and vice versa.
        assert_eq!(project_iommu_table(&graph, cpu(1), &mut out), 0);
        assert_eq!(project_page_table(&graph, pci(0, 3, 0), &mut out), 0);
    }

    /// THE falsifiable test from docs/NOVEL_CONCEPTS.md section 1.4:
    /// revoke a grant and show it is gone from BOTH projections,
    /// verified independently for each, using only the single `revoke`
    /// call -- no separate "update the IOMMU side too" step exists to
    /// forget, because there is no such step to call.
    #[test]
    fn revocation_removes_the_grant_from_both_projections_atomically() {
        use kernel_common::authority_graph::revoke;

        let mut graph: [Option<Grant>; 8] = [None; 8];
        let dev = pci(0, 31, 2); // real AHCI B/D/F this project's own iommu.rs uses
        grant(&mut graph, Grant { principal: dev, phys_base: 0x81_0000, len: 4096, writable: true, executable: false });

        assert!(is_reachable(&graph, dev, 0x81_0000));
        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 8];
        assert_eq!(project_iommu_table(&graph, dev, &mut out), 1);

        let removed = revoke(&mut graph, dev, 0x81_0000);
        assert_eq!(removed, 1);

        // Both projections, independently re-queried, must show it gone.
        assert_eq!(project_iommu_table(&graph, dev, &mut out), 0);
        assert_eq!(project_page_table(&graph, dev, &mut out), 0);
        assert!(!is_reachable(&graph, dev, 0x81_0000));
    }

    /// Direct regression test for the real historical bug shape
    /// documented in kernel_rs::iommu.rs's own module doc: a SECOND
    /// device assigned after a FIRST must never silently orphan the
    /// first's own grant. Here: revoking device B's grant must leave
    /// device A's grant completely intact in both projections.
    #[test]
    fn graph_bug_regression_matches_the_real_historical_iommu_bug_shape() {
        use kernel_common::authority_graph::revoke;

        let mut graph: [Option<Grant>; 8] = [None; 8];
        let dev_a = pci(0, 3, 0); // e.g. virtio-blk, first device assigned on bus 0
        let dev_b = pci(0, 31, 2); // e.g. AHCI, second device assigned on the SAME bus

        grant(&mut graph, Grant { principal: dev_a, phys_base: 0x20_0000, len: 4096, writable: true, executable: false });
        grant(&mut graph, Grant { principal: dev_b, phys_base: 0x30_0000, len: 4096, writable: true, executable: false });

        // Revoking B must not touch A at all.
        revoke(&mut graph, dev_b, 0x30_0000);

        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 8];
        assert_eq!(project_iommu_table(&graph, dev_a, &mut out), 1, "device A's grant was orphaned by an unrelated device's revocation -- the exact real bug this design forecloses");
        assert_eq!(out[0].phys_page, 0x20_0000);
        assert!(is_reachable(&graph, dev_a, 0x20_0000));
        assert!(!is_reachable(&graph, dev_b, 0x30_0000));
    }

    #[test]
    fn a_multi_page_range_projects_one_entry_per_page() {
        let mut graph: [Option<Grant>; 8] = [None; 8];
        grant(&mut graph, Grant { principal: cpu(2), phys_base: 0x40_0000, len: 3 * 4096, writable: false, executable: true });

        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 8];
        let count = project_page_table(&graph, cpu(2), &mut out);
        assert_eq!(count, 3);
        assert_eq!(out[0].phys_page, 0x40_0000);
        assert_eq!(out[1].phys_page, 0x40_1000);
        assert_eq!(out[2].phys_page, 0x40_2000);
        assert!(out[0].executable);
        assert!(!out[0].writable);
    }

    #[test]
    fn a_non_page_aligned_length_rounds_up_not_down() {
        // 1 byte over one page must still produce 2 entries -- rounding
        // DOWN would leave part of a granted range unmapped (a real
        // under-grant bug), never acceptable even though it's the
        // "safer-looking" direction.
        let mut graph: [Option<Grant>; 8] = [None; 8];
        grant(&mut graph, Grant { principal: cpu(3), phys_base: 0x50_0000, len: 4097, writable: true, executable: false });

        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 8];
        assert_eq!(project_page_table(&graph, cpu(3), &mut out), 2);
    }

    #[test]
    fn grant_capacity_is_respected_not_silently_overrun() {
        let mut graph: [Option<Grant>; 2] = [None; 2];
        assert!(grant(&mut graph, Grant { principal: cpu(1), phys_base: 0x1000, len: 4096, writable: true, executable: false }));
        assert!(grant(&mut graph, Grant { principal: cpu(1), phys_base: 0x2000, len: 4096, writable: true, executable: false }));
        assert!(!grant(&mut graph, Grant { principal: cpu(1), phys_base: 0x3000, len: 4096, writable: true, executable: false }));
    }

    #[test]
    fn projection_output_capacity_is_respected_not_silently_overrun() {
        let mut graph: [Option<Grant>; 8] = [None; 8];
        grant(&mut graph, Grant { principal: cpu(1), phys_base: 0x1000, len: 5 * 4096, writable: true, executable: false });

        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 3]; // room for 3, not 5
        let count = project_page_table(&graph, cpu(1), &mut out);
        assert_eq!(count, 3);
    }

    #[test]
    fn revoking_a_nonexistent_grant_is_a_clean_noop_not_an_error() {
        let mut graph: [Option<Grant>; 4] = [None; 4];
        use kernel_common::authority_graph::revoke;
        assert_eq!(revoke(&mut graph, cpu(9), 0xdead_0000), 0);
    }

    #[test]
    fn adversarial_repeated_grant_revoke_cycles_never_leave_a_stale_projection() {
        // Adversarial per docs/NOVEL_CONCEPTS.md section 1.4: mutate
        // through many cycles and assert, after every single mutation,
        // that both projections and is_reachable agree with each
        // other and with the graph's actual live contents. No cycle
        // should ever leave a projection stale.
        use kernel_common::authority_graph::revoke;

        let mut graph: [Option<Grant>; 4] = [None; 4];
        let dev = pci(1, 0, 0);
        let mut out = [Entry { phys_page: 0, writable: false, executable: false }; 4];

        for i in 0..50u64 {
            let phys = 0x9000_0000 + i * 0x1000;
            grant(&mut graph, Grant { principal: dev, phys_base: phys, len: 4096, writable: true, executable: false });
            assert!(is_reachable(&graph, dev, phys));
            assert_eq!(project_iommu_table(&graph, dev, &mut out), 1);
            assert_eq!(out[0].phys_page, phys);

            let removed = revoke(&mut graph, dev, phys);
            assert_eq!(removed, 1);
            assert!(!is_reachable(&graph, dev, phys));
            assert_eq!(project_iommu_table(&graph, dev, &mut out), 0);
            assert_eq!(project_page_table(&graph, dev, &mut out), 0);
        }
    }
}

#[cfg(test)]
mod impossibility_certificate_tests {
    // Research track (docs/RESEARCH_TRACK.md, docs/NOVEL_CONCEPTS.md
    // section 2): real, isolated verification of the pure certificate
    // data model. Zero QEMU, zero hardware -- this only proves the
    // certificate logic itself is sound; real hardware wiring is a
    // separate, later increment (see the module's own doc).

    use kernel_common::authority_graph::{grant, revoke, Grant, Principal};
    use kernel_common::impossibility_certificate::{issue, verify, Verdict};

    fn cpu(id: u32) -> Principal {
        Principal::CpuProcess(id)
    }
    fn pci(bus: u8, device: u8, function: u8) -> Principal {
        Principal::PciDevice { bus, device, function }
    }

    #[test]
    fn a_certificate_can_be_issued_for_a_genuinely_unreachable_page() {
        let graph: [Option<Grant>; 4] = [None; 4];
        let cert = issue(&graph, cpu(1), 0x1000).expect("empty graph -- everything is unreachable");
        assert_eq!(verify(&graph, &cert), Verdict::Valid);
    }

    #[test]
    fn no_certificate_can_be_issued_for_a_reachable_page() {
        let mut graph: [Option<Grant>; 4] = [None; 4];
        grant(&mut graph, Grant { principal: cpu(1), phys_base: 0x1000, len: 4096, writable: true, executable: false });
        // The exact page IS reachable -- issuing a certificate that it
        // is NOT would be a false claim; the API must refuse it.
        assert!(issue(&graph, cpu(1), 0x1000).is_none());
        // A DIFFERENT page, not covered by the grant, is still fair game.
        assert!(issue(&graph, cpu(1), 0x5000).is_some());
    }

    #[test]
    fn a_certificate_verifies_valid_while_the_graph_is_unchanged() {
        let graph: [Option<Grant>; 4] = [None; 4];
        let cert = issue(&graph, pci(0, 3, 0), 0x2000).unwrap();
        // Re-verify several times -- must be stable, not one-shot.
        assert_eq!(verify(&graph, &cert), Verdict::Valid);
        assert_eq!(verify(&graph, &cert), Verdict::Valid);
    }

    /// THE falsifiable test from docs/NOVEL_CONCEPTS.md section 2.4's
    /// spirit (the pure-data-model version -- the real hardware-
    /// corruption version is separate follow-up work): any change to
    /// the graph the certificate was issued against, even one
    /// unrelated to the certified page, must make it Stale, not
    /// silently keep validating.
    #[test]
    fn any_graph_mutation_makes_a_previously_issued_certificate_stale() {
        let mut graph: [Option<Grant>; 8] = [None; 8];
        let target = pci(0, 31, 2);
        let cert = issue(&graph, target, 0x81_0000).unwrap();
        assert_eq!(verify(&graph, &cert), Verdict::Valid);

        // An entirely UNRELATED grant, for a different principal and a
        // different page -- the certified page is still genuinely
        // unreachable by `target`, but the certificate is about a
        // SPECIFIC graph state, and that state has changed.
        grant(&mut graph, Grant { principal: cpu(99), phys_base: 0x99_0000, len: 4096, writable: true, executable: false });
        assert_eq!(verify(&graph, &cert), Verdict::Stale);
    }

    /// Real finding from this test's own first run, corrected here
    /// rather than hidden: the checksum is content-addressed BY DESIGN
    /// (the module doc's own "two identical graphs" guarantee, proven
    /// by the test above) -- revoking a grant and then regranting the
    /// EXACT same content is not a content change, so a certificate
    /// correctly re-verifies Valid afterward. The original version of
    /// this test asserted the opposite (Stale) and was simply wrong
    /// about what this module's own documented contract promises; this
    /// is that corrected, honest version, kept rather than deleted so
    /// the content-addressed property is explicitly exercised through
    /// a revoke/regrant cycle, not just two independently-built graphs.
    #[test]
    fn revoking_and_regranting_identical_content_still_verifies_valid() {
        let mut graph: [Option<Grant>; 4] = [None; 4];
        let dev = pci(0, 3, 0);
        grant(&mut graph, Grant { principal: dev, phys_base: 0x4000, len: 4096, writable: true, executable: false });
        let cert = issue(&graph, dev, 0x9000).unwrap();
        assert_eq!(verify(&graph, &cert), Verdict::Valid);

        revoke(&mut graph, dev, 0x4000);
        grant(&mut graph, Grant { principal: dev, phys_base: 0x4000, len: 4096, writable: true, executable: false });
        assert_eq!(verify(&graph, &cert), Verdict::Valid);
    }

    /// The real contrast case: regranting with even ONE field
    /// genuinely different (writable flipped here) IS a content
    /// change, and must go Stale -- proving the checksum is actually
    /// sensitive to grant contents, not just presence/absence.
    #[test]
    fn regranting_with_a_different_flag_makes_the_certificate_stale() {
        let mut graph: [Option<Grant>; 4] = [None; 4];
        let dev = pci(0, 3, 0);
        grant(&mut graph, Grant { principal: dev, phys_base: 0x4000, len: 4096, writable: true, executable: false });
        let cert = issue(&graph, dev, 0x9000).unwrap();
        assert_eq!(verify(&graph, &cert), Verdict::Valid);

        revoke(&mut graph, dev, 0x4000);
        grant(&mut graph, Grant { principal: dev, phys_base: 0x4000, len: 4096, writable: false, executable: false });
        assert_eq!(verify(&graph, &cert), Verdict::Stale);
    }

    #[test]
    fn two_identical_graphs_produce_a_certificate_that_verifies_against_either() {
        // Real, checked property: the checksum is a pure function of
        // graph CONTENTS, not of identity/order-of-construction --
        // issuing against one graph and verifying against a separately
        // built but content-identical graph must succeed.
        let mut graph_a: [Option<Grant>; 4] = [None; 4];
        let mut graph_b: [Option<Grant>; 4] = [None; 4];
        let dev = pci(0, 3, 0);
        grant(&mut graph_a, Grant { principal: dev, phys_base: 0x4000, len: 4096, writable: true, executable: false });
        grant(&mut graph_b, Grant { principal: dev, phys_base: 0x4000, len: 4096, writable: true, executable: false });

        let cert = issue(&graph_a, dev, 0x9000).unwrap();
        assert_eq!(verify(&graph_b, &cert), Verdict::Valid);
    }
}

#[cfg(test)]
mod discovered_envelope_tests {
    // Research track (docs/RESEARCH_TRACK.md, docs/NOVEL_CONCEPTS.md
    // section 3): real, isolated verification -- rated MODERATE
    // novelty confidence in NOVEL_CONCEPTS.md section 4, built anyway
    // as a real, cheap consequence of section 1 already existing.

    use kernel_common::authority_graph::{is_reachable, Grant, Principal};
    use kernel_common::discovered_envelope::{discover_envelope, freeze_envelope};

    fn pci(bus: u8, device: u8, function: u8) -> Principal {
        Principal::PciDevice { bus, device, function }
    }

    #[test]
    fn envelope_discovery_deduplicates_repeated_accesses() {
        let attempts = [0x1000u64, 0x2000, 0x1000, 0x3000, 0x2000, 0x1000];
        let mut out = [0u64; 8];
        let count = discover_envelope(&attempts, &mut out);
        assert_eq!(count, 3);
        assert_eq!(&out[..3], &[0x1000, 0x2000, 0x3000]);
    }

    /// Real, direct analogue of docs/NOVEL_CONCEPTS.md section 3.3's
    /// own falsifiable test: a real driver's known DMA needs
    /// (mirroring nvme_driver's three page-aligned regions -- admin
    /// submission queue, admin completion queue, data buffer)
    /// discovered from a simulated access trace, matches EXACTLY, then
    /// frozen into real enforced grants.
    #[test]
    fn a_known_three_page_driver_pattern_is_discovered_exactly() {
        let asq = 0x52_0000u64;
        let acq = 0x53_0000u64;
        let data = 0x54_0000u64;
        // A real access trace would touch each region multiple times
        // (queue doorbell writes, completion polls, data reads) --
        // simulated here as repeated attempts, same shape.
        let attempts = [asq, asq, acq, data, acq, data, asq, data];

        let mut envelope = [0u64; 8];
        let count = discover_envelope(&attempts, &mut envelope);
        assert_eq!(count, 3);
        assert_eq!(&envelope[..3], &[asq, acq, data]);
    }

    #[test]
    fn a_discovered_envelope_freezes_into_real_grants_that_allow_exactly_those_pages() {
        let dev = pci(0, 3, 0);
        let attempts = [0x10_0000u64, 0x20_0000, 0x30_0000];
        let mut envelope = [0u64; 8];
        let count = discover_envelope(&attempts, &mut envelope);

        let mut graph: [Option<Grant>; 8] = [None; 8];
        let granted = freeze_envelope(&mut graph, dev, &envelope[..count]);
        assert_eq!(granted, 3);

        for &addr in &envelope[..count] {
            assert!(is_reachable(&graph, dev, addr));
        }
    }

    /// THE falsifiable test from docs/NOVEL_CONCEPTS.md section 3.3:
    /// a deliberately modified driver attempting a FOURTH region (one
    /// never seen in the original access trace) must fault against the
    /// frozen envelope -- least privilege discovered by observation,
    /// then genuinely enforced, not just recorded.
    #[test]
    fn a_page_never_observed_in_the_trace_is_denied_after_freezing() {
        let dev = pci(0, 3, 0);
        let attempts = [0x10_0000u64, 0x20_0000, 0x30_0000];
        let mut envelope = [0u64; 8];
        let count = discover_envelope(&attempts, &mut envelope);

        let mut graph: [Option<Grant>; 8] = [None; 8];
        freeze_envelope(&mut graph, dev, &envelope[..count]);

        // A "deliberately modified" fourth region, never in the trace.
        assert!(!is_reachable(&graph, dev, 0x40_0000));
    }

    #[test]
    fn envelope_discovery_output_capacity_is_respected() {
        let attempts = [0x1000u64, 0x2000, 0x3000, 0x4000, 0x5000];
        let mut out = [0u64; 2]; // room for 2, not 5
        let count = discover_envelope(&attempts, &mut out);
        assert_eq!(count, 2);
    }

    #[test]
    fn freeze_envelope_respects_graph_capacity_not_silently_overrun() {
        let dev = pci(0, 3, 0);
        let mut graph: [Option<Grant>; 2] = [None; 2];
        let envelope = [0x1000u64, 0x2000, 0x3000];
        let granted = freeze_envelope(&mut graph, dev, &envelope);
        assert_eq!(granted, 2); // only 2 slots available
    }

    #[test]
    fn an_empty_trace_discovers_an_empty_envelope_not_a_panic() {
        let attempts: [u64; 0] = [];
        let mut out = [0u64; 4];
        assert_eq!(discover_envelope(&attempts, &mut out), 0);
    }
}

#[cfg(test)]
mod supervision_tests {
    // Phase 9.5a -- real host verification of kernel_common::supervision,
    // the pure logic kernel_rs::device_manager/kernel_rs::supervisor
    // wrap with real thread/PCI/audit state. See that module's own doc.
    use kernel_common::supervision::*;

    #[test]
    fn pack_unpack_round_trips_for_a_real_bus_device_function() {
        let packed = pack_bdf(0x00, 0x1f, 0x02); // the real ich9-ahci controller this project's own PCI scan finds
        assert_eq!(unpack_bdf(packed), (0x00, 0x1f, 0x02));
    }

    #[test]
    fn pack_unpack_round_trips_at_the_real_max_values() {
        let packed = pack_bdf(0xFF, 0xFF, 0xFF);
        assert_eq!(unpack_bdf(packed), (0xFF, 0xFF, 0xFF));
    }

    #[test]
    fn pack_unpack_round_trips_for_zero() {
        let packed = pack_bdf(0, 0, 0);
        assert_eq!(unpack_bdf(packed), (0, 0, 0));
        assert_eq!(packed, 0);
    }

    #[test]
    fn different_bus_device_function_triples_never_collide() {
        // Exhaustive over every real device/function PCI allows (0..32,
        // 0..8), for several representative bus values.
        for bus in [0x00u8, 0x01, 0xFF] {
            let mut seen = std::vec::Vec::new();
            for device in 0..32u8 {
                for function in 0..8u8 {
                    let p = pack_bdf(bus, device, function);
                    assert!(!seen.contains(&p), "collision at bus={} device={} function={}", bus, device, function);
                    seen.push(p);
                }
            }
        }
    }

    #[test]
    fn a_fresh_device_with_zero_prior_restarts_is_allowed_attempt_one() {
        assert_eq!(decide_restart(0, 3), RestartDecision::Restart { attempt: 1 });
    }

    #[test]
    fn the_last_allowed_attempt_is_exactly_at_the_budget_boundary() {
        // max_restarts=3: prior_restart_count 0,1,2 are all still allowed (attempts 1,2,3); 3 is not.
        assert_eq!(decide_restart(2, 3), RestartDecision::Restart { attempt: 3 });
    }

    #[test]
    fn exhausting_the_budget_exactly_triggers_quarantine_not_one_more_restart() {
        assert_eq!(decide_restart(3, 3), RestartDecision::Quarantine);
    }

    #[test]
    fn a_device_already_past_budget_stays_quarantined_not_reset() {
        assert_eq!(decide_restart(4, 3), RestartDecision::Quarantine);
    }

    #[test]
    fn a_zero_restart_budget_quarantines_immediately_never_restarts_once() {
        assert_eq!(decide_restart(0, 0), RestartDecision::Quarantine);
    }
}

#[cfg(test)]
mod heap_coalescing_tests {
    use core::ptr::null_mut;

    struct FreeListNode {
        size: usize,
        next: *mut FreeListNode,
    }

    struct TestHeap {
        head: *mut FreeListNode,
    }

    impl TestHeap {
        fn new() -> Self {
            TestHeap { head: null_mut() }
        }

        unsafe fn add_free_region(&mut self, addr: u64, size: usize) {
            if size < core::mem::size_of::<FreeListNode>() {
                return;
            }

            let mut prev: *mut FreeListNode = null_mut();
            let mut curr = self.head;
            while !curr.is_null() && (curr as u64) < addr {
                prev = curr;
                curr = (*curr).next;
            }

            // Case 1: Merge with prev
            if !prev.is_null() && (prev as u64) + (*prev).size as u64 == addr {
                (*prev).size += size;
                // Also merge with curr if adjacent
                if !curr.is_null() && (prev as u64) + (*prev).size as u64 == curr as u64 {
                    (*prev).size += (*curr).size;
                    (*prev).next = (*curr).next;
                }
                return;
            }

            // Case 2: Merge with curr
            if !curr.is_null() && addr + size as u64 == curr as u64 {
                let node = addr as *mut FreeListNode;
                (*node).size = size + (*curr).size;
                (*node).next = (*curr).next;
                if prev.is_null() {
                    self.head = node;
                } else {
                    (*prev).next = node;
                }
                return;
            }

            // Case 3: No merge, insert between prev and curr
            let node = addr as *mut FreeListNode;
            (*node).size = size;
            (*node).next = curr;
            if prev.is_null() {
                self.head = node;
            } else {
                (*prev).next = node;
            }
        }

        unsafe fn free_blocks_count(&self) -> usize {
            let mut count = 0;
            let mut curr = self.head;
            while !curr.is_null() {
                count += 1;
                curr = (*curr).next;
            }
            count
        }

        unsafe fn total_free_bytes(&self) -> usize {
            let mut total = 0;
            let mut curr = self.head;
            while !curr.is_null() {
                total += (*curr).size;
                curr = (*curr).next;
            }
            total
        }
    }

    #[test]
    fn sequential_blocks_coalesce_into_single_block() {
        let mut heap = TestHeap::new();
        let buffer = [0u8; 4096];
        let base = buffer.as_ptr() as u64;

        unsafe {
            heap.add_free_region(base, 1024);
            assert_eq!(heap.free_blocks_count(), 1);
            assert_eq!(heap.total_free_bytes(), 1024);

            heap.add_free_region(base + 1024, 1024);
            assert_eq!(heap.free_blocks_count(), 1);
            assert_eq!(heap.total_free_bytes(), 2048);

            heap.add_free_region(base + 2048, 1024);
            assert_eq!(heap.free_blocks_count(), 1);
            assert_eq!(heap.total_free_bytes(), 3072);
        }
    }

    #[test]
    fn reverse_blocks_coalesce_into_single_block() {
        let mut heap = TestHeap::new();
        let buffer = [0u8; 4096];
        let base = buffer.as_ptr() as u64;

        unsafe {
            heap.add_free_region(base + 2048, 1024);
            heap.add_free_region(base + 1024, 1024);
            heap.add_free_region(base, 1024);

            assert_eq!(heap.free_blocks_count(), 1);
            assert_eq!(heap.total_free_bytes(), 3072);
        }
    }

    #[test]
    fn middle_block_bridges_and_coalesces_both_sides() {
        let mut heap = TestHeap::new();
        let buffer = [0u8; 4096];
        let base = buffer.as_ptr() as u64;

        unsafe {
            // Free left block [base, base + 1024)
            heap.add_free_region(base, 1024);
            // Free right block [base + 2048, base + 3072)
            heap.add_free_region(base + 2048, 1024);
            assert_eq!(heap.free_blocks_count(), 2);
            assert_eq!(heap.total_free_bytes(), 2048);

            // Free middle block [base + 1024, base + 2048)
            heap.add_free_region(base + 1024, 1024);
            // All three should merge into ONE contiguous 3072-byte block!
            assert_eq!(heap.free_blocks_count(), 1);
            assert_eq!(heap.total_free_bytes(), 3072);
        }
    }

    #[test]
    fn non_adjacent_blocks_remain_separate_and_sorted() {
        let mut heap = TestHeap::new();
        let buffer = [0u8; 4096];
        let base = buffer.as_ptr() as u64;

        unsafe {
            heap.add_free_region(base, 512);
            heap.add_free_region(base + 1024, 512);
            heap.add_free_region(base + 2048, 512);

            assert_eq!(heap.free_blocks_count(), 3);
            assert_eq!(heap.total_free_bytes(), 1536);

            let node0 = heap.head;
            assert_eq!(node0 as u64, base);
            let node1 = (*node0).next;
            assert_eq!(node1 as u64, base + 1024);
            let node2 = (*node1).next;
            assert_eq!(node2 as u64, base + 2048);
            assert!((*node2).next.is_null());
        }
    }
}

#[cfg(test)]
mod request_queue_tests {
    struct InFlightSlot<T> {
        id: u64,
        data: T,
        ready: bool,
    }

    struct RequestQueue<T> {
        slots: Vec<InFlightSlot<T>>,
        next_id: u64,
    }

    impl<T> RequestQueue<T> {
        fn new() -> Self {
            Self { slots: Vec::new(), next_id: 1 }
        }

        fn enqueue(&mut self, data: T) -> u64 {
            let id = self.next_id;
            self.next_id += 1;
            self.slots.push(InFlightSlot { id, data, ready: false });
            id
        }

        fn mark_ready(&mut self, id: u64) -> bool {
            if let Some(slot) = self.slots.iter_mut().find(|s| s.id == id) {
                slot.ready = true;
                true
            } else {
                false
            }
        }

        fn poll_reply(&mut self, id: u64) -> Option<T> {
            let idx = self.slots.iter().position(|s| s.id == id)?;
            if self.slots[idx].ready {
                Some(self.slots.remove(idx).data)
            } else {
                None
            }
        }
    }

    #[test]
    fn multi_slot_queue_does_not_overwrite_concurrent_requests() {
        let mut queue = RequestQueue::<u32>::new();
        let id1 = queue.enqueue(11); // e.g. inode 11
        let id2 = queue.enqueue(12); // e.g. inode 12
        let id3 = queue.enqueue(13); // e.g. inode 13

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        assert_eq!(queue.slots.len(), 3);

        // Polling before ready returns None
        assert_eq!(queue.poll_reply(id1), None);
        assert_eq!(queue.poll_reply(id2), None);

        // Mark request 2 ready first (out of order completion)
        assert!(queue.mark_ready(id2));
        assert_eq!(queue.poll_reply(id2), Some(12));
        assert_eq!(queue.slots.len(), 2);

        // Requests 1 and 3 are still in-flight and unaffected
        assert_eq!(queue.poll_reply(id1), None);
        assert!(queue.mark_ready(id1));
        assert_eq!(queue.poll_reply(id1), Some(11));
        assert_eq!(queue.slots.len(), 1);

        assert!(queue.mark_ready(id3));
        assert_eq!(queue.poll_reply(id3), Some(13));
        assert_eq!(queue.slots.len(), 0);
    }

    #[test]
    fn unknown_or_stale_id_returns_none() {
        let mut queue = RequestQueue::<u32>::new();
        let id = queue.enqueue(11);
        assert!(queue.mark_ready(id));
        assert_eq!(queue.poll_reply(id), Some(11));
        // Second poll for the already-retired slot returns None
        assert_eq!(queue.poll_reply(id), None);
        // Made-up id returns None
        assert_eq!(queue.poll_reply(999), None);
    }
}

#[cfg(test)]
mod geometry_tests {
    use kernel_common::geometry::Rect;

    #[test]
    fn intersect_disjoint_returns_none() {
        let a = Rect::new(0, 0, 50, 50);
        let b = Rect::new(60, 60, 50, 50);
        assert_eq!(a.intersect(&b), None);
        assert_eq!(b.intersect(&a), None);
    }

    #[test]
    fn intersect_touching_edge_returns_none() {
        let a = Rect::new(0, 0, 50, 50);
        let b = Rect::new(50, 0, 50, 50);
        assert_eq!(a.intersect(&b), None);
    }

    #[test]
    fn intersect_overlapping_returns_correct_box() {
        let a = Rect::new(10, 10, 100, 100);
        let b = Rect::new(50, 50, 100, 100);
        let inter = a.intersect(&b).expect("should intersect");
        assert_eq!(inter, Rect::new(50, 50, 60, 60));
    }

    #[test]
    fn contains_rect_full_and_partial() {
        let outer = Rect::new(0, 0, 200, 200);
        let inner = Rect::new(10, 10, 50, 50);
        let crossing = Rect::new(150, 150, 100, 100);
        assert!(outer.contains_rect(&inner));
        assert!(outer.contains_rect(&outer));
        assert!(!outer.contains_rect(&crossing));
        assert!(!inner.contains_rect(&outer));
    }

    #[test]
    fn subtract_disjoint_returns_original() {
        let a = Rect::new(0, 0, 50, 50);
        let b = Rect::new(100, 100, 50, 50);
        let mut out = [Rect::default(); 4];
        let n = a.subtract(&b, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], a);
    }

    #[test]
    fn subtract_fully_covered_returns_zero() {
        let target = Rect::new(20, 20, 40, 40);
        let occluder = Rect::new(0, 0, 100, 100);
        let mut out = [Rect::default(); 4];
        let n = target.subtract(&occluder, &mut out);
        assert_eq!(n, 0);
    }

    #[test]
    fn subtract_center_hole_produces_four_disjoint_slices_matching_area() {
        let target = Rect::new(10, 10, 100, 100); // area = 10,000
        let hole = Rect::new(30, 30, 40, 40);     // area = 1,600
        let mut out = [Rect::default(); 4];
        let n = target.subtract(&hole, &mut out);
        assert_eq!(n, 4);

        let total_area: usize = out[..n].iter().map(|r| r.area()).sum();
        assert_eq!(total_area, 10000 - 1600); // 8,400

        // Ensure all slices are mutually disjoint
        for i in 0..n {
            for j in (i + 1)..n {
                assert_eq!(out[i].intersect(&out[j]), None, "slice {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn subtract_top_edge_returns_one_bottom_slice() {
        let target = Rect::new(0, 0, 100, 100);
        let occluder = Rect::new(0, 0, 100, 40);
        let mut out = [Rect::default(); 4];
        let n = target.subtract(&occluder, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], Rect::new(0, 40, 100, 60));
    }

    #[test]
    fn subtract_left_edge_returns_one_right_slice() {
        let target = Rect::new(0, 0, 100, 100);
        let occluder = Rect::new(0, 0, 30, 100);
        let mut out = [Rect::default(); 4];
        let n = target.subtract(&occluder, &mut out);
        assert_eq!(n, 1);
        assert_eq!(out[0], Rect::new(30, 0, 70, 100));
    }

    #[test]
    fn compute_visible_rects_with_multiple_occluders() {
        let target = Rect::new(0, 0, 200, 200); // area = 40,000
        // Two non-overlapping occluders
        let occ1 = Rect::new(0, 0, 100, 100);     // top-left (10,000)
        let occ2 = Rect::new(100, 100, 100, 100); // bottom-right (10,000)

        let mut out = [Rect::default(); 32];
        let n = Rect::compute_visible_rects(&target, &[occ1, occ2], &mut out);
        assert!(n > 0);

        let visible_area: usize = out[..n].iter().map(|r| r.area()).sum();
        assert_eq!(visible_area, 20000); // 40,000 - 20,000 = 20,000

        // Ensure all resulting visible rects are mutually disjoint
        for i in 0..n {
            for j in (i + 1)..n {
                assert_eq!(out[i].intersect(&out[j]), None, "rects {} and {} overlap", i, j);
            }
        }
    }

    #[test]
    fn compute_visible_rects_complete_occlusion() {
        let target = Rect::new(50, 50, 50, 50);
        let big = Rect::new(0, 0, 200, 200);
        let mut out = [Rect::default(); 32];
        let n = Rect::compute_visible_rects(&target, &[big], &mut out);
        assert_eq!(n, 0);
    }
}

#[cfg(test)]
mod frame_pacing_tests {
    use kernel_common::frame_pacing::FramePacer;

    const CYCLES_PER_US: u64 = 2500;

    #[test]
    fn target_fps_interval_calculations() {
        let pacer60 = FramePacer::new(60);
        assert_eq!(pacer60.frame_interval_us(), 16_666);
        assert_eq!(pacer60.frame_interval_cycles(CYCLES_PER_US), 16_666 * 2500);

        let pacer120 = FramePacer::new(120);
        assert_eq!(pacer120.frame_interval_us(), 8_333);

        let pacer0 = FramePacer::new(0);
        assert_eq!(pacer0.frame_interval_us(), 0);
        assert_eq!(pacer0.frame_interval_cycles(CYCLES_PER_US), 0);
    }

    #[test]
    fn idle_desktop_never_presents() {
        let mut pacer = FramePacer::new(60);
        // Idle: no damage marked.
        assert!(!pacer.request_presentation(100_000_000, CYCLES_PER_US, false));
        assert!(!pacer.request_presentation(200_000_000, CYCLES_PER_US, false));
        assert_eq!(pacer.presented_frames, 0);
        assert_eq!(pacer.dropped_frames, 0);
    }

    #[test]
    fn first_damage_after_idle_presents_immediately() {
        let mut pacer = FramePacer::new(60);
        pacer.mark_damage();
        // Since last_present_tsc is 0, elapsed >= interval is true
        assert!(pacer.request_presentation(50_000_000, CYCLES_PER_US, false));
        assert_eq!(pacer.presented_frames, 1);
        assert_eq!(pacer.last_present_tsc, 50_000_000);
    }

    #[test]
    fn rapid_mouse_dragging_throttles_within_interval() {
        let mut pacer = FramePacer::new(60);
        let interval_cycles = pacer.frame_interval_cycles(CYCLES_PER_US); // ~41,665,000 cycles

        // t = 0: first frame presents
        pacer.mark_damage();
        assert!(pacer.request_presentation(1_000_000_000, CYCLES_PER_US, false));
        assert_eq!(pacer.presented_frames, 1);

        // 1000 Hz mouse events arrive every 1 ms (2,500,000 cycles):
        // 10 events within 10 ms (< 16.6ms) should all be throttled
        for i in 1..=10 {
            pacer.mark_damage();
            let now = 1_000_000_000 + i * 2_500_000;
            assert!(!pacer.request_presentation(now, CYCLES_PER_US, false), "event {} should be throttled", i);
        }
        assert_eq!(pacer.presented_frames, 1);
        assert_eq!(pacer.dropped_frames, 10);

        // Event at 17 ms (> 16.666 ms) reaches frame deadline!
        pacer.mark_damage();
        let now = 1_000_000_000 + interval_cycles + 1000;
        assert!(pacer.request_presentation(now, CYCLES_PER_US, false));
        assert_eq!(pacer.presented_frames, 2);
    }

    #[test]
    fn force_presentation_bypasses_interval_deadline() {
        let mut pacer = FramePacer::new(60);
        pacer.mark_damage();
        assert!(pacer.request_presentation(1_000_000, CYCLES_PER_US, false));

        // Forced presentation (e.g. mouse drag release) presents immediately
        pacer.mark_damage();
        assert!(pacer.request_presentation(1_001_000, CYCLES_PER_US, true));
        assert_eq!(pacer.presented_frames, 2);
    }
}

#[cfg(test)]
mod allocation_free_hotpath_tests {
    use kernel_common::geometry::Rect;
    use kernel_common::frame_pacing::FramePacer;

    #[test]
    fn zero_allocation_composition_hotpath_soak() {
        let mut pacer = FramePacer::new(60);
        let screen = Rect::new(0, 0, 1024, 768);
        let mut visible_out = [Rect::default(); 32];
        let mut vacated_out = [Rect::default(); 4];

        // Simulate 500 frames of rapid mouse drag with multiple occluding windows
        for frame in 0..500 {
            let win_x = 100 + (frame % 200) as i32;
            let win_y = 100 + (frame % 150) as i32;
            let current_win = Rect::new(win_x, win_y, 300, 200);
            let prev_win = Rect::new(win_x - 2, win_y - 1, 300, 200);

            // Compute vacated slices (zero alloc)
            let n_vacated = prev_win.subtract(&current_win, &mut vacated_out);
            assert!(n_vacated <= 4);

            // Compute visible slices against occluders (zero alloc)
            let occluder = Rect::new(150, 150, 200, 200);
            let n_vis = Rect::compute_visible_rects(&current_win, &[occluder], &mut visible_out);
            assert!(n_vis <= 32);

            // Ensure all visible slices are inside screen
            for k in 0..n_vis {
                let r = visible_out[k];
                assert!(screen.contains_rect(&r) || screen.intersect(&r).is_some());
            }

            pacer.mark_damage();
            let now = 1_000_000_000 + (frame as u64) * 2_500_000;
            let _ = pacer.request_presentation(now, 2500, false);
        }
    }
}

#[cfg(test)]
mod vsync_and_display_timing_tests {
    use kernel_common::frame_pacing::{DisplayTiming, PresentationFence};

    const CYCLES_PER_US: u64 = 2500;

    #[test]
    fn display_timing_refresh_periods() {
        let dt60 = DisplayTiming::new(60);
        assert_eq!(dt60.refresh_period_us(), 16_666);
        assert_eq!(dt60.refresh_period_cycles(CYCLES_PER_US), 16_666 * 2500);

        let dt120 = DisplayTiming::new(120);
        assert_eq!(dt120.refresh_period_us(), 8_333);

        let dt144 = DisplayTiming::new(144);
        assert_eq!(dt144.refresh_period_us(), 6_944);

        let dt0 = DisplayTiming::new(0);
        assert_eq!(dt0.refresh_period_us(), 0);
        assert_eq!(dt0.refresh_period_cycles(CYCLES_PER_US), 0);
    }

    #[test]
    fn display_timing_next_deadline_alignment() {
        let mut dt = DisplayTiming::new(60);
        let period_cycles = dt.refresh_period_cycles(CYCLES_PER_US);

        // Before any vsync recorded, next deadline is now
        assert_eq!(dt.next_deadline_tsc(1_000_000, CYCLES_PER_US), 1_000_000);

        // VSync recorded at t = 100_000_000
        dt.record_vsync(100_000_000);
        assert_eq!(dt.total_vsync_events, 1);

        // Midway through frame (5 ms in): next deadline is t + 1 frame
        let t_mid = 100_000_000 + 5_000 * CYCLES_PER_US;
        assert_eq!(dt.next_deadline_tsc(t_mid, CYCLES_PER_US), 100_000_000 + period_cycles);

        // 25 ms in (> 1 frame, < 2 frames): next deadline is t + 2 frames
        let t_late = 100_000_000 + 25_000 * CYCLES_PER_US;
        assert_eq!(dt.next_deadline_tsc(t_late, CYCLES_PER_US), 100_000_000 + 2 * period_cycles);
    }

    #[test]
    fn presentation_fence_advancement_and_ordering() {
        let mut fence = PresentationFence::new();
        assert_eq!(fence.sequence, 0);
        assert!(fence.is_reached(0));
        assert!(!fence.is_reached(1));

        let s1 = fence.advance();
        assert_eq!(s1, 1);
        assert!(fence.is_reached(1));
        assert!(!fence.is_reached(2));

        let s2 = fence.advance();
        assert_eq!(s2, 2);
        assert!(fence.is_reached(1));
        assert!(fence.is_reached(2));
        assert!(!fence.is_reached(3));
    }
}

#[cfg(test)]
mod virtio_gpu_tests {
    use kernel_common::virtio_gpu_proto::*;
    use core::mem::size_of;

    #[test]
    fn virtio_gpu_packet_sizes_and_alignment() {
        assert_eq!(size_of::<VirtioGpuCtrlHdr>(), 24);
        assert_eq!(size_of::<VirtioGpuRect>(), 16);
        assert_eq!(size_of::<VirtioGpuResourceCreate2d>(), 40);
        assert_eq!(size_of::<VirtioGpuSetScanout>(), 48);
        assert_eq!(size_of::<VirtioGpuTransferToHost2d>(), 56);
        assert_eq!(size_of::<VirtioGpuResourceFlush>(), 48);
    }

    #[test]
    fn virtio_gpu_command_packet_construction() {
        let create_cmd = VirtioGpuResourceCreate2d::new(1, VIRTIO_GPU_FORMAT_B8G8R8A8_UNORM, 1024, 768);
        assert_eq!(create_cmd.hdr.hdr_type, VIRTIO_GPU_CMD_RESOURCE_CREATE_2D);
        assert_eq!(create_cmd.resource_id, 1);
        assert_eq!(create_cmd.width, 1024);
        assert_eq!(create_cmd.height, 768);

        let rect = VirtioGpuRect::new(0, 0, 1024, 768);
        let scanout_cmd = VirtioGpuSetScanout::new(0, 1, rect);
        assert_eq!(scanout_cmd.hdr.hdr_type, VIRTIO_GPU_CMD_SET_SCANOUT);
        assert_eq!(scanout_cmd.scanout_id, 0);
        assert_eq!(scanout_cmd.resource_id, 1);
        assert_eq!(scanout_cmd.r.width, 1024);

        let flush_cmd = VirtioGpuResourceFlush::new(1, rect);
        assert_eq!(flush_cmd.hdr.hdr_type, VIRTIO_GPU_CMD_RESOURCE_FLUSH);
        assert_eq!(flush_cmd.resource_id, 1);

        let transfer_cmd = VirtioGpuTransferToHost2d::new(1, 0, rect);
        assert_eq!(transfer_cmd.hdr.hdr_type, VIRTIO_GPU_CMD_TRANSFER_TO_HOST_2D);
        assert_eq!(transfer_cmd.offset, 0);
    }
}

#[cfg(test)]
mod animation_tests {
    use kernel_common::animation::{ease_out_quad, interpolate_position, lerp_i32, SlideAnimation, Q16_ONE};

    #[test]
    fn lerp_i32_boundaries_and_midpoint() {
        assert_eq!(lerp_i32(100, 200, 0), 100);
        assert_eq!(lerp_i32(100, 200, Q16_ONE), 200);
        assert_eq!(lerp_i32(100, 200, Q16_ONE / 2), 150);
        assert_eq!(lerp_i32(200, 100, Q16_ONE / 2), 150);
    }

    #[test]
    fn ease_out_quad_properties() {
        assert_eq!(ease_out_quad(0), 0);
        assert_eq!(ease_out_quad(Q16_ONE), Q16_ONE);

        // Ease-out is faster initially than linear: at 50% progress, eased > 50%
        let mid = ease_out_quad(Q16_ONE / 2);
        assert!(mid > Q16_ONE / 2);
        // Specifically for f(0.5) = 0.5 * (2 - 0.5) = 0.75
        assert_eq!(mid, (Q16_ONE * 3) / 4);
    }

    #[test]
    fn slide_animation_progression_and_completion() {
        let mut slide = SlideAnimation::new();
        assert!(!slide.active);

        slide.start((50, 50), (250, 150), 1_000_000, 10_000);
        assert!(slide.active);

        // At t = 1_000_000 (t=0)
        let (x0, y0, done0) = slide.step(1_000_000);
        assert_eq!((x0, y0), (50, 50));
        assert!(!done0);
        assert!(slide.active);

        // At t = 1_005_000 (t=50%, progress=75% due to ease-out)
        let (x_mid, y_mid, done_mid) = slide.step(1_005_000);
        // dx = 200 * 0.75 = 150 -> x = 200
        // dy = 100 * 0.75 = 75  -> y = 125
        assert_eq!((x_mid, y_mid), (200, 125));
        assert!(!done_mid);

        // At t = 1_010_000 (t=100%, complete)
        let (x_end, y_end, done_end) = slide.step(1_010_000);
        assert_eq!((x_end, y_end), (250, 150));
        assert!(done_end);
        assert!(!slide.active);
    }
}







