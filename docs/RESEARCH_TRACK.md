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
| §2 — Physical impossibility certificates | Not started (depends on §1) | — |
| §3 — Hardware-fault-discovered least privilege | Not started (depends on §1) | — |

---

## §1 — Authority graph = hardware translation structures

**Goal of this increment:** a pure, host-tested module proving the CORE claim — that one authority structure can generate both a page-table-shaped projection and an IOMMU-table-shaped projection, and that revoking an entry in the source structure removes it from BOTH projections atomically (no separate update path to forget). Zero hardware, zero QEMU — this increment only has to prove the DATA MODEL is sound; wiring it to real `vmm.rs`/`iommu.rs` page tables is a later, separate increment once this one holds up.

**Real risk being tested:** whether "one source, two projections" is actually simpler/safer than "two structures kept in sync" for the specific shapes x86_64 4-level paging and VT-d context/PASID tables need — not assumed, checked.

**Result: the data-model prototype is real, built, and verified.** `kernel_common::authority_graph` (pure, `#![no_std]`, no heap — caller-provided fixed-capacity storage, same discipline as `driver_registry`/`madt`): `Grant`/`Principal`/`Entry` types, `grant`/`revoke` mutating the one source structure, `project_page_table`/`project_iommu_table` as pure reads of it into the SAME `Entry` shape (grounded in the real, already-documented fact that VT-d second-level translation uses the identical page-table bit format as CPU paging — `kernel_rs::iommu::assign_device`'s own existing comment, not an assumption invented for this).

**10 real host tests, all passing** (`host_tests`, zero QEMU): a fresh grant reachable by its own projection; projections correctly filtered by principal; **the falsifiable revocation test from `docs/NOVEL_CONCEPTS.md` §1.4** (revoke a grant, independently re-query both projections, both show it gone, using only the single `revoke` call — no second "update the IOMMU side too" step exists to forget); **a direct regression test reproducing the exact shape of the real historical `iommu.rs` bug** (device B's revocation must never orphan device A's still-live grant — the actual failure mode `assign_device`'s own module doc records); multi-page range projection; non-page-aligned length rounds up (never down — an under-grant would be the dangerous direction); capacity bounds respected on both grant storage and projection output; a clean no-op on revoking a nonexistent grant; and a 50-cycle adversarial grant/revoke loop asserting both projections and `is_reachable` agree with the graph's live contents after every single mutation, not just at the end.

**A real bug was found and fixed by this project's own first test run, not glossed over:** the first version of `project_page_table`/`project_iommu_table` only *documented* that a caller should pass a matching principal kind (CPU vs. PCI device) — nothing *enforced* it. The test `projections_are_filtered_by_principal_not_shared_across_principals` failed immediately, catching exactly the class of gap this whole concept exists to foreclose: software believing a distinction that nothing actually checks. Fixed by making both projectors match on `Principal` and return 0 unconditionally for the wrong kind — a real, enforced type-level guarantee now, not a comment. Documented in the module's own doc comment, not silently corrected.

**Verified not to affect the real kernel:** `kernel_rs` (which depends on `kernel_common`) still builds clean and boots clean (`scripts/test-boot.ps1` passes) with this module present but completely unwired — exactly the isolation this increment was scoped to.

**Deliberately not done in this increment:** wiring this against real `vmm.rs`/`iommu.rs` page tables on actual hardware — that's what would prove real hardware atomicity (§1's own doc is explicit this prototype only proves the data model). Also not done: PASID/scalable-mode IOMMU table shapes (this prototype's IOMMU projection targets the same simple second-level shape `iommu.rs` already implements, not the full VT-d spec). §2 and §3 remain not started, as both depend on §1 holding up first.

(Updated as work lands below.)
