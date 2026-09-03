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
