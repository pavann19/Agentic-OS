# M5 Prompt: Kernel Heap And Core Runtime

## Prompt For Antigravity

Implement milestone `M5: Heap And Core Runtime`.

Inspect PMM/VMM APIs, `kernel/graphics.c`, `kernel/print.c`, and existing memory/string helper code before editing.

Goal: add safe dynamic allocation and basic runtime primitives required by later kernel subsystems.

## Required Changes

- Add kernel heap API:
  - `void heap_init(void)`
  - `void* kmalloc(uint64_t size)`
  - `void* kcalloc(uint64_t count, uint64_t size)`
  - `void* kmalloc_aligned(uint64_t size, uint64_t alignment)`
  - `void kfree(void* ptr)`
- Start with a simple allocator that is correct and debuggable. A bump allocator plus clear non-freeing behavior is acceptable only if documented; otherwise implement a small free-list allocator.
- Heap must allocate from the VMM heap range, backed by PMM pages.
- Add core runtime primitives:
  - `memcpy`
  - `memmove`
  - `memset`
  - `memcmp`
  - `strlen`
  - `strcmp`
  - bounded string copy/compare functions
- Move duplicated local memory functions into shared runtime code.
- Change graphics back buffer allocation to use contiguous virtual memory backed by pages through VMM/heap, not assumed contiguous physical pages.
- Add allocation failure logs and panic only when the caller cannot safely recover.

## Non-Goals

- Do not implement user-space `malloc`.
- Do not add filesystem or scheduler.
- Do not optimize allocator performance before correctness and tests.

## Verification

Run:

```sh
make clean all
make test-boot
make test-host
```

Host tests should cover allocator basics and runtime primitives where feasible.

## Acceptance Criteria

- Kernel boot still reaches existing checkpoints.
- Heap initialization emits serial evidence.
- `kmalloc`, `kcalloc`, alignment, and `kfree` behavior is documented.
- Back buffer no longer requires contiguous physical allocation.
- Host tests cover common allocator and memory primitive cases.
