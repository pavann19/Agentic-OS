# M3 Prompt: Physical Memory Manager Redesign

## Prompt For Antigravity

Implement milestone `M3: Physical Memory Manager Redesign`.

Inspect `include/bootinfo.h`, `kernel/memory.c`, `kernel/memory.h`, `kernel/paging.c`, `kernel/graphics.c`, and serial boot evidence before editing.

Goal: replace prototype physical page ownership with a trustworthy PMM that does not treat firmware holes as usable memory.

## Required Changes

- Stop reporting or using highest physical address as usable memory.
- Track:
  - total physical span
  - total usable memory
  - reserved memory
  - free pages
  - used pages
- Initialize all pages as used, then free only pages from `EfiConventionalMemory`.
- Explicitly reserve:
  - page zero
  - kernel physical range from `BootInfo`
  - boot info structure
  - UEFI memory map buffer
  - font header and glyph buffer
  - framebuffer MMIO range
  - PMM bitmap pages
  - page tables as they are allocated
- Add public API:
  - `void* pmm_alloc_page(void)`
  - `void* pmm_alloc_pages(uint64_t count, uint32_t flags)`
  - `void pmm_free_page(void* page)`
  - `void pmm_reserve_range(uint64_t start, uint64_t length, const char* reason)`
  - `PmmStats pmm_get_stats(void)`
  - `void pmm_dump_stats(void)`
- `pmm_free_page` must reject:
  - null/page zero
  - unaligned pointers
  - out-of-range pages
  - already-free pages
  - reserved pages
- `pmm_alloc_page` must not imply contiguity.
- `pmm_alloc_pages` must be the only API for contiguous physical runs.
- PMM must log stats over serial after initialization.

## Non-Goals

- Do not implement virtual memory redesign in this stage except adapting calls needed for PMM correctness.
- Do not implement heap.
- Do not add user mode.

## Verification

Run:

```sh
make clean all
make test-boot
```

Add host-side tests if feasible by isolating bitmap/range logic. If host tests are added, run:

```sh
make test-host
```

## Acceptance Criteria

- Serial log shows PMM statistics.
- Page zero is reserved.
- Kernel, boot info, memory map, framebuffer, and bitmap ranges cannot be allocated.
- Sparse memory maps do not make holes allocatable.
- Normal page allocation and contiguous allocation are separate code paths.
