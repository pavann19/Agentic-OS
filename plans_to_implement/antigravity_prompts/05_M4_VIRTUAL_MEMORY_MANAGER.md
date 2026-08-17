# M4 Prompt: Virtual Memory Manager

## Prompt For Antigravity

Implement milestone `M4: Virtual Memory Manager`.

Inspect `kernel/paging.c`, `kernel/paging.h`, `kernel/memory.c`, `kernel/linker.ld`, `include/bootinfo.h`, and exception logging before editing.

Goal: replace broad identity mapping with a defined kernel virtual memory layout.

## Required Changes

- Define a documented virtual memory layout in code comments and docs:
  - null guard unmapped
  - temporary low identity map for transition only
  - higher-half kernel mapping
  - physical direct-map window
  - MMIO window for framebuffer
  - kernel heap window for M5
  - user-space range reserved for M8
- Add public API:
  - `int vmm_init(BootInfo* bootInfo)`
  - `int vmm_map_page(uint64_t vaddr, uint64_t paddr, uint64_t flags)`
  - `int vmm_unmap_page(uint64_t vaddr)`
  - `int vmm_map_range(uint64_t vaddr, uint64_t paddr, uint64_t length, uint64_t flags)`
  - `AddressSpace* vmm_create_address_space(void)`
  - `void vmm_switch_address_space(AddressSpace* space)`
- Keep page zero unmapped.
- Map only required regions:
  - kernel text/rodata/data/bss
  - framebuffer MMIO
  - boot info and font memory
  - physical direct-map window needed by PMM/VMM
  - page tables
- Apply page permissions:
  - kernel text executable and read-only when practical
  - rodata read-only
  - data/bss writable
  - no user bit on kernel pages
  - NX where CPU support is available and already detected safely
- Add TLB invalidation for unmap/remap.
- Add a controlled null-dereference fault test path guarded by a compile-time or test flag.

## Non-Goals

- Do not implement user mode yet.
- Do not build heap except reserving the virtual range.
- Do not add APIC/timer work unless needed for testing.

## Verification

Run:

```sh
make clean all
make test-boot
```

If a fault test target is added, run it and store serial logs separately under `_evidence/latest/null-fault-serial.log`.

## Acceptance Criteria

- Kernel boots without broad identity-map dependency.
- Virtual address zero is unmapped.
- Required regions remain accessible after switching page tables.
- Page fault logs identify null dereference when the test is enabled.
- `make test-boot` still passes in normal mode.
