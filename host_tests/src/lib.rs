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
