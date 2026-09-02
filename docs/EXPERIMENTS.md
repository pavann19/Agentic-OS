# Experiments — tested in isolation, not yet merged

Per explicit instruction: novel ideas get built and verified standalone first; nothing here is wired into the real boot path until told to merge it. Each entry states what was tested, the real evidence, and exactly what a merge would touch — kept small on purpose, so "plug into main" stays a small, reviewable diff, not a rewrite.

## 1. Generic PCI driver-matching registry (`kernel_common::driver_registry`)

**What it is:** a real, pure, table-driven replacement for the five near-identical `find_X`/`spawn_if_present` functions `kernel_rs/src/{virtio_blk,virtio_net,ahci,nvme,e1000}.rs` each currently hand-roll. One real matcher (`match_all`), two match-rule kinds (`VendorDevice`, `Class` — exactly the two kinds this project's own five real drivers already use), operating on caller-provided data with zero heap allocation, zero hardware access, zero `unsafe`.

**Where it lives right now:** `kernel_common/src/driver_registry.rs` — a real module in the shared, hardware-independent crate `kernel_rs` already depends on, but **not referenced anywhere in `kernel_rs`'s own boot path.** It compiles and is tested; it does nothing at runtime yet.

**Real evidence, not asserted:** `host_tests/src/lib.rs::driver_registry_tests`, 6 real tests, all passing (`cargo test` — no QEMU, seconds not minutes):
- Each match-rule kind matches correctly (and correctly *fails* to match a near-miss — e.g. `virtio-net`'s own device ID against `virtio-blk`'s rule).
- The full 5-driver table, matched against a realistic 7-device list (including 2 devices that match nothing, mirroring a real `pci::enumerate()` result), finds exactly the right 5 devices, in order, each with the correct handler.
- **A real, found-along-the-way limitation of TODAY's code**, proven by a test, not just described: every existing `find_X` function uses `.find(...)`, which stops at the FIRST matching device — if a machine ever has two NVMe controllers, the second gets silently ignored, no driver, no log line. `match_all` has no such limitation (a dedicated test, `two_devices_of_the_same_kind_are_both_matched_not_just_the_first`, proves it).
- Output-capacity bounds are respected (no panic, no silent overrun) and non-matching devices are skipped cleanly, not treated as errors.

**Why this is a real improvement, not just different code:** adding a sixth driver today means writing a new ~10-line `find_X` function AND remembering to call it in `main.rs`. With the registry, it means adding one row to one table. And it closes the two-controllers-of-the-same-kind gap for free.

### The merge, concretely — what "plug into main" would actually touch

This is deliberately small. Three real changes, no restructuring of the drivers themselves (`virtio_blk.rs`/`virtio_net.rs`/`ahci.rs`/`nvme.rs`/`e1000.rs`'s own `spawn_if_present` bodies don't change at all):

1. **Delete the five `find_X` functions** (`find_virtio_blk`, `find_virtio_net`, `find_ahci`, `find_nvme`, `find_e1000`) — their logic moves into one static table.
2. **Add one table** in `main.rs` (or a new small `kernel_rs/src/drivers.rs`):
   ```rust
   enum Spawn { VirtioBlk, VirtioNet, Ahci, Nvme, E1000 }
   const DRIVER_TABLE: &[DriverEntry<Spawn>] = &[
       DriverEntry { name: "virtio_blk", rule: MatchRule::VendorDevice(0x1AF4, 0x1042), handler: Spawn::VirtioBlk },
       DriverEntry { name: "virtio_net", rule: MatchRule::VendorDevice(0x1AF4, 0x1041), handler: Spawn::VirtioNet },
       DriverEntry { name: "ahci",       rule: MatchRule::VendorDevice(0x8086, 0x2922), handler: Spawn::Ahci },
       DriverEntry { name: "nvme",       rule: MatchRule::Class(0x01, 0x08, 0x02),       handler: Spawn::Nvme },
       DriverEntry { name: "e1000",      rule: MatchRule::VendorDevice(0x8086, 0x100E),  handler: Spawn::E1000 },
   ];
   ```
3. **Replace the five sequential calls** in `main.rs`:
   ```rust
   virtio_blk::spawn_if_present(&pci_devices);
   virtio_net::spawn_if_present(&pci_devices);
   ahci::spawn_if_present(&pci_devices);
   nvme::spawn_if_present(&pci_devices);
   e1000::spawn_if_present(&pci_devices);
   ```
   with one real loop over `match_all`'s output, dispatching on the matched `Spawn` variant. Each `spawn_if_present` itself would lose its own internal `find_X` call/early-return (the match already happened) and instead take the already-found `PciDevice` directly — a small signature change, same body otherwise.

**Real risk this merge carries, stated honestly:** every driver's own `spawn_if_present` currently does its OWN device lookup and its OWN "not found" log line (`"VIRTIO_NET: no modern virtio-net device found"` etc.) — moving the lookup into the shared table means that specific log line's wording would need to move too (logged once, generically, by the dispatch loop for anything the table finds no rule for — or per-driver by checking whether that driver's own handler appears in the matched set). Small, mechanical, but a real behavior change to double-check against existing test scripts that grep for those exact strings (`scripts/test-*.ps1`) before merging, not just compile-check.

**Verdict:** genuinely improves the codebase (less duplication, closes a real multi-device gap) and the merge is real but small — a good candidate to merge when asked, not urgent enough to have merged unprompted.
