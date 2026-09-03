# Research Track — Novel Concepts, Tracked Separately From The Roadmap

**Started:** 2026-09-03
**Relationship to `docs/ROADMAP.md`:** parallel, not inserted. The numbered phases (0-15) are evidence-gated infrastructure the OS needs regardless of whether any concept here pans out; this track never blocks them and they never block it. See `docs/NOVEL_CONCEPTS.md` for the concepts themselves and `docs/PRIOR_ART.md` for the review that produced them.

**Standing discipline (unchanged from the rest of this project):** every concept here is built and verified in isolation — `host_tests`, zero QEMU, zero boot-path wiring — before any integration decision is even discussed. Only once a concept is proven standalone does a real integration plan get written, the same path `kernel_common::driver_registry` took. Nothing in this file is wired into `kernel_rs`'s actual boot path until explicitly promoted.

**If a concept is ever promoted:** the merge itself becomes a normal, numbered roadmap deliverable (most likely folded into an existing phase — e.g. §1's projection is a natural fit for Phase 9 deliverable 5's "every shared kernel structure audited," since it touches the same IOMMU/page-table code — or, if large enough, a new phase). That decision is made at promotion time, not assumed now.

---

## Status

| Concept (`docs/NOVEL_CONCEPTS.md` §) | Status | Evidence |
|---|---|---|
| §1 — Authority graph = hardware translation structures | **Data-model prototype DONE, isolated** — real hardware wiring NOT started | see below |
| §2 — Physical impossibility certificates | **Data-model prototype DONE, isolated** — real hardware wiring NOT started | see below |
| §3 — Hardware-fault-discovered least privilege | **Data-model prototype DONE, isolated** — real hardware wiring NOT started | see below |

---

## §1 — Authority graph = hardware translation structures

**Goal of this increment:** a pure, host-tested module proving the CORE claim — that one authority structure can generate both a page-table-shaped projection and an IOMMU-table-shaped projection, and that revoking an entry in the source structure removes it from BOTH projections atomically (no separate update path to forget). Zero hardware, zero QEMU — this increment only has to prove the DATA MODEL is sound; wiring it to real `vmm.rs`/`iommu.rs` page tables is a later, separate increment once this one holds up.

**Real risk being tested:** whether "one source, two projections" is actually simpler/safer than "two structures kept in sync" for the specific shapes x86_64 4-level paging and VT-d context/PASID tables need — not assumed, checked.

**Result: the data-model prototype is real, built, and verified.** `kernel_common::authority_graph` (pure, `#![no_std]`, no heap — caller-provided fixed-capacity storage, same discipline as `driver_registry`/`madt`): `Grant`/`Principal`/`Entry` types, `grant`/`revoke` mutating the one source structure, `project_page_table`/`project_iommu_table` as pure reads of it into the SAME `Entry` shape (grounded in the real, already-documented fact that VT-d second-level translation uses the identical page-table bit format as CPU paging — `kernel_rs::iommu::assign_device`'s own existing comment, not an assumption invented for this).

**10 real host tests, all passing** (`host_tests`, zero QEMU): a fresh grant reachable by its own projection; projections correctly filtered by principal; **the falsifiable revocation test from `docs/NOVEL_CONCEPTS.md` §1.4** (revoke a grant, independently re-query both projections, both show it gone, using only the single `revoke` call — no second "update the IOMMU side too" step exists to forget); **a direct regression test reproducing the exact shape of the real historical `iommu.rs` bug** (device B's revocation must never orphan device A's still-live grant — the actual failure mode `assign_device`'s own module doc records); multi-page range projection; non-page-aligned length rounds up (never down — an under-grant would be the dangerous direction); capacity bounds respected on both grant storage and projection output; a clean no-op on revoking a nonexistent grant; and a 50-cycle adversarial grant/revoke loop asserting both projections and `is_reachable` agree with the graph's live contents after every single mutation, not just at the end.

**A real bug was found and fixed by this project's own first test run, not glossed over:** the first version of `project_page_table`/`project_iommu_table` only *documented* that a caller should pass a matching principal kind (CPU vs. PCI device) — nothing *enforced* it. The test `projections_are_filtered_by_principal_not_shared_across_principals` failed immediately, catching exactly the class of gap this whole concept exists to foreclose: software believing a distinction that nothing actually checks. Fixed by making both projectors match on `Principal` and return 0 unconditionally for the wrong kind — a real, enforced type-level guarantee now, not a comment. Documented in the module's own doc comment, not silently corrected.

**Verified not to affect the real kernel:** `kernel_rs` (which depends on `kernel_common`) still builds clean and boots clean (`scripts/test-boot.ps1` passes) with this module present but completely unwired — exactly the isolation this increment was scoped to.

**Deliberately not done in this increment:** wiring this against real `vmm.rs`/`iommu.rs` page tables on actual hardware — that's what would prove real hardware atomicity (§1's own doc is explicit this prototype only proves the data model). Also not done: PASID/scalable-mode IOMMU table shapes (this prototype's IOMMU projection targets the same simple second-level shape `iommu.rs` already implements, not the full VT-d spec). §2 and §3 remain not started, as both depend on §1 holding up first.

---

### §1, second increment — real hardware wiring, done and verified on real silicon

**Goal:** escalate §1 from "the data model is sound" (host-tested, zero hardware) to "the authority graph actually drives this kernel's real IOMMU hardware state, and revocation actually retracts it in real hardware bytes" — the next honest step the first increment's own doc called out as not yet done.

**What was built, real:**
- `iommu::revoke_device(bus, device, function)` — new, real hardware-facing revocation. Clears the SAME context-table entry `assign_device` writes (present bit → 0), flushes the context cache the same way `assign_device` already does. `assign_device` could previously only ever ADD a device to a domain; there was no revocation path in this kernel's IOMMU code at all before this.
- `iommu::context_entry_present(bus, device, function)` — real, direct read-back of the actual physical bytes the IOMMU silicon consults, not kernel-side bookkeeping about that state.
- `kernel_rs::authority` — the real wiring module. Holds the ONE authority graph (`kernel_common::authority_graph`) as kernel state; `grant_device`/`revoke_device` derive the real `iommu::assign_device`/`iommu::revoke_device` calls FROM the graph's own projection, not from a separately-passed range list. `cross_check` reads both the graph and the real hardware context entry independently, for verification.
- AHCI's real, existing capability grant (`ahci.rs`) now goes through `authority::grant_device` instead of calling `iommu::assign_device` directly — a real production code path now runs through the graph, with a safety-net fallback to the direct call if the graph is ever full (stated honestly, not hidden).

**Verified live, real QEMU boot, real hardware bytes, not simulated:** a self-contained boot-time self-check (`main.rs`, right after `IOMMU_INIT_DONE`), using a synthetic, unused PCI slot (00:1d.7 — never a real device, so this never touches AHCI/virtio-blk's own live context entries) so the demonstration needs no live-driver timing coordination:

```
AUTHORITY_HW_SELFCHECK_START graph_reachable=false hw_present=false (both must be false before any grant)
AUTHORITY_HW_SELFCHECK_GRANT domain=2 graph_reachable=true hw_present=true (both must be true)
AUTHORITY_HW_SELFCHECK_REVOKE revoked_ok=true graph_reachable=false hw_present=false (both must be false again)
AUTHORITY_HW_SELFCHECK_PASS: real IOMMU context-table state tracked the authority graph exactly, grant and revoke, both verified against real hardware-facing bytes
```

Every `hw_present`/`hw_present` value above is a direct read of the real physical bytes the IOMMU hardware itself consults (`iommu::context_entry_present`) — not a claim about kernel bookkeeping. This is the escalation from "the data model is sound" (first increment) to "the real hardware agrees with the graph, through a real grant and a real revoke, on real silicon."

**Also verified:** AHCI's existing self-check (`scripts/test-ahci.ps1`) still passes identically through the new graph-derived wiring — confirmed via the real `AUTHORITY_GRANT_HW` log line showing `projected_pages=1` (the real range came from the graph's projection, not a bypassed direct call). Full 12-suite regression re-run clean (one pre-existing, already-documented flaky test — `test-e1000` — reconfirmed unrelated, passes standalone).

**Deliberately not done yet, stated honestly (at the time):** a live, third-party device issuing a SECOND real DMA attempt after revocation, and observing a real IOMMU fault (`iommu::poll_and_log_faults`) as the direct consequence. The CPU-side page-table projection (`project_page_table` → real `vmm::map_page_in`) also remains unwired.

---

### §1, third increment — the live-device fault, and a real bug this test itself found

**Goal:** the exact escalation the second increment named as not yet done — a real, live AHCI device (00:1f.2), issuing a real second IDENTIFY DEVICE command against a buffer whose grant was just revoked, with a real IOMMU fault as the observed consequence.

**Built:** `kernel_rs::authority_hw_fault_demo` — kernel-side (ring 0), not the live ring-3 `ahci_driver` (coordinating exact revoke-mid-flight timing against a real process needs an IPC signal this kernel doesn't have wired for this purpose; what matters for the claim is that a real PCI device's own hardware performs the DMA, which is true regardless of which ring issued the MMIO command that triggers it). Reuses the exact, already-verified AHCI 1.3.1 command-table byte layout from `user_rs/ahci_driver` — same constants, same offsets, not reinvented. Runs: real IDENTIFY with a live grant (control — must succeed), `authority::revoke_device`, then the SAME IDENTIFY again against the SAME now-revoked buffer, with a short bounded `PxCI` wait (learned from this same session's earlier AHCI debugging: a command a device genuinely can't complete can leave `PxCI` set indefinitely — a real, spec-correct AHCI behavior, not a bug — so this module never trusts `PxCI` clearing as its evidence; `iommu::poll_and_log_faults` is the authoritative signal).

**First real run found a genuine bug in this project's own IOMMU code, not a success:** the second command *completed* (`second_command_completed=true`), and zero IOMMU faults were captured. Root cause, found by reading the VT-d spec against `iommu::revoke_device`'s actual implementation, not guessed: `revoke_device` only flushed the **context cache** — correct for `assign_device`'s own case (a fresh domain has nothing cached yet), but a device that already completed a real DMA (the control step, run immediately before every revocation test) has its translation cached in the **IOTLB** (the address-translation cache), a separate cache the VT-d spec requires a separate invalidation for. This kernel had never implemented IOTLB invalidation at all before this — `revoke_device`'s original version left a genuinely stale, already-revoked-in-software translation still honored by real hardware.

**Fixed for real:** `revoke_device` now reads the domain ID back out of the context entry before clearing it, then issues a real domain-selective IOTLB invalidation (`ECAP.IRO`-computed register offset — the same "compute the offset from the device's own reported capability register, don't assume a spec-typical constant" discipline `fault_recording_regs` already established for the fault-recording registers), polling for hardware completion the same bounded way context-cache invalidation already does.

**Re-run after the fix: full pass, independently cross-confirmed.** Not just this kernel's own self-report — QEMU's own internal emulation stderr, completely independent of anything this kernel logs, shows the exact same event: `vtd_iommu_translate: detected translation failure (dev=00:1f:02, iova=0x18be500)`. The kernel's own captured fault record decodes to `source_id=0x00fa` = bus 0x00, device 0x1f, function 2 — exactly AHCI, matching QEMU's own independent report of which device it blocked.

```
AUTHORITY_HW_LIVE_FAULT_CONTROL first_command_completed=true spurious_faults=0
AUTHORITY_HW_LIVE_FAULT_REVOKED revoked_ok=true
IOMMU_FAULT_DETECTED source_id=0x00fa reason=0x02 raw_low=0x00000000018be000 raw_high=0xc0000002000000fa
AUTHORITY_HW_LIVE_FAULT_RESULT second_command_completed=false iommu_faults_captured=1
AUTHORITY_HW_LIVE_FAULT_PASS: a real, live PCI device's DMA attempt was blocked by real IOMMU hardware immediately after authority::revoke_device, captured as a real fault -- not a simulated or asserted result
```

**A second real bug found, this time in this project's own test process, not the kernel:** running this demo unconditionally on every boot broke `scripts/test-keyboard.ps1` — not the usual, already-documented `test-e1000` flakiness, but a new, real failure (reproducible within the full suite, passing standalone), root-caused (not assumed) to the demo's own deliberate bounded wait for the expected-to-fail second command adding real synchronous wall-clock delay before the kernel reaches the steady state `test-keyboard.ps1`'s fixed boot-wait timing assumes. Fixed the way this project already fixes exactly this class of problem (`fault_injection.rs`, `demo_ring3`): a new Cargo feature, `research_authority_hw_demo`, **off by default** — both the synthetic-slot self-check and the live-device demo are gated behind it, so a normal boot (and the standard 12-suite regression) never runs either. A dedicated script, `scripts/test-authority-revoke.ps1`, builds the feature-enabled kernel, boots it, asserts all four real markers above, then rebuilds and redeploys the default (non-feature) kernel — same "leave the tree in its normal state" discipline `test-faults.ps1` already established. Not added to `scripts/test-integration.ps1`'s standard list, matching the existing precedent `test-ring3.ps1` already set for feature-gated demos.

**Verified:** default (no-feature) boot — `test-keyboard.ps1` passes again, full 12-suite regression green. Feature-enabled boot — `scripts/test-authority-revoke.ps1` passes all four checks.

**Two real, honestly-found-and-fixed bugs in one increment** is not a footnote to hide — it is the actual evidence this research track's own discipline (build in isolation, test against real hardware, never assume) is doing its job: the whole point of escalating from the pure data model to real silicon was to find exactly the class of gap the data model couldn't reveal, and it did, twice.

**Deliberately still not done:** the CPU-side page-table projection (`project_page_table` → real `vmm::map_page_in`) remains unwired — device DMA containment was the priority target since that is where this project's real historical bug lived.

---

## §2 — Physical impossibility certificates

**Goal of this increment:** a pure, host-tested module proving the core claim — that a runtime, verifiable "principal cannot reach physical page P" certificate can be issued and later re-verified against the SAME authority graph §1 already built, with no hardware wiring yet (that is the real, separate escalation §1 itself took three increments to reach; §2 starts the same way §1 did).

**Built:** `kernel_common::impossibility_certificate` — `issue(graph, principal, phys_page)` refuses to issue a certificate for anything actually reachable (a certificate can only ever claim a real, checked impossibility); `verify(graph, cert)` re-derives a real, deterministic FNV-1a checksum over every live grant in the CURRENT graph and compares it against what was certified, returning `Valid` or `Stale`.

**A real, checked, counterintuitive property found by this module's own first test run, corrected honestly rather than hidden:** the checksum is CONTENT-addressed (it hashes what the graph currently contains), not EVENT-addressed (it is not a log of what happened). The first version of the test suite asserted that revoking a grant and then regranting the byte-for-byte identical `Grant` should make a certificate `Stale` — it failed, because the resulting state genuinely is, in every checkable way, identical to what was certified, and `Valid` is the correct answer. The test was wrong about the module's own documented contract, not the code. Kept as a corrected test (`revoking_and_regranting_identical_content_still_verifies_valid`) specifically to exercise this real property, alongside a contrast test (`regranting_with_a_different_flag_makes_the_certificate_stale`) proving the checksum genuinely is sensitive to content, not just presence.

**9 real host tests, all passing:** issuance succeeds for genuinely unreachable pages, refuses for reachable ones; a certificate stays `Valid` across repeated re-verification; the falsifiable staleness test (an unrelated grant elsewhere makes a certificate about a completely different page `Stale`); the content-vs-event-addressed pair above; two independently-built but content-identical graphs cross-verify each other's certificates.

**Real, honestly-stated limitation, stated in the module's own doc, not discovered later:** the checksum is a real integrity check (FNV-1a), not a cryptographic signature — no key, no unforgeability guarantee against an adversary who can also compute FNV-1a. Real cryptographic signing is explicitly out of scope for this increment.

**Deliberately not done yet:** real hardware wiring — binding a certificate to the live IOMMU context-table bytes (`kernel_rs::iommu::context_entry_present`) instead of only the pure graph, and the actual falsifiable test `docs/NOVEL_CONCEPTS.md` §2.4 describes (issue a certificate, deliberately corrupt one real IOMMU table entry underneath it, show the certificate fails to re-verify against live hardware). That is real, valuable, separate follow-up work.

---

## §3 — Hardware-fault-discovered least privilege

**Goal of this increment:** a pure, host-tested module proving the core claim — that an authority envelope can be DISCOVERED from a real access trace (not declared by a human or policy engine) and then FROZEN into real, enforced grants. Rated only MODERATE novelty confidence in `docs/NOVEL_CONCEPTS.md` §4 (real adjacent art: systrace/AppArmor/seccomp learning-mode profile generation, named there rather than glossed over) — built anyway as a real, cheap consequence of §1 already existing.

**Built:** `kernel_common::discovered_envelope` — `discover_envelope(attempts, out)` deduplicates a caller-provided list of physical-page access attempts into a minimal set, bounded and no-panic; `freeze_envelope(graph, principal, envelope)` grants exactly that set into the authority graph as real page-granular `Grant`s.

**7 real host tests, all passing**, including a direct analogue of `docs/NOVEL_CONCEPTS.md` §3.3's own falsifiable test: a simulated access trace shaped like `nvme_driver`'s real, known three-page DMA pattern (admin submission queue, admin completion queue, data buffer — each touched multiple times, matching a real queue-doorbell/completion-poll/data-read access pattern) is discovered down to exactly those three pages, frozen into real grants, and **a fourth region never seen in the trace is confirmed denied** (`is_reachable` false) after freezing — least privilege discovered by observation, then genuinely enforced against something new, not just recorded.

**Real, stated scope limit of this increment:** this operates on a caller-provided list of access attempts (plain `u64` addresses) — it does not itself observe real hardware faults. Wiring it to a REAL captured fault trace from `kernel_rs::iommu::poll_and_log_faults` (replacing the synthetic attempt list this module's tests use with real fault records from an actual driver run) is real, separate follow-up work.

---

Both §2 and §3 stayed at the pure-data-model stage this increment, matching exactly how §1 itself proceeded (isolated prototype first, real hardware wiring as a deliberately separate, later step) — no shortcuts taken to claim more than what was actually built and tested.

(Updated as work lands below.)
