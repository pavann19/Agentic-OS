# Research Track — Novel Concepts, Tracked Separately From The Roadmap

**Started:** 2026-09-03
**Relationship to `docs/ROADMAP.md`:** parallel, not inserted. The numbered phases (0-15) are evidence-gated infrastructure the OS needs regardless of whether any concept here pans out; this track never blocks them and they never block it. See `docs/NOVEL_CONCEPTS.md` for the concepts themselves and `docs/PRIOR_ART.md` for the review that produced them.

**Standing discipline (unchanged from the rest of this project):** every concept here is built and verified in isolation — `host_tests`, zero QEMU, zero boot-path wiring — before any integration decision is even discussed. Only once a concept is proven standalone does a real integration plan get written, the same path `kernel_common::driver_registry` took. Nothing in this file is wired into `kernel_rs`'s actual boot path until explicitly promoted.

**If a concept is ever promoted:** the merge itself becomes a normal, numbered roadmap deliverable (most likely folded into an existing phase — e.g. §1's projection is a natural fit for Phase 9 deliverable 5's "every shared kernel structure audited," since it touches the same IOMMU/page-table code — or, if large enough, a new phase). That decision is made at promotion time, not assumed now.

---

## Status

| Concept (`docs/NOVEL_CONCEPTS.md` §) | Status | Evidence |
|---|---|---|
| §1 — Authority graph = hardware translation structures | **IN PROGRESS** — isolated prototype started 2026-09-03 | see below |
| §2 — Physical impossibility certificates | Not started (depends on §1) | — |
| §3 — Hardware-fault-discovered least privilege | Not started (depends on §1) | — |

---

## §1 — Authority graph = hardware translation structures

**Goal of this increment:** a pure, host-tested module proving the CORE claim — that one authority structure can generate both a page-table-shaped projection and an IOMMU-table-shaped projection, and that revoking an entry in the source structure removes it from BOTH projections atomically (no separate update path to forget). Zero hardware, zero QEMU — this increment only has to prove the DATA MODEL is sound; wiring it to real `vmm.rs`/`iommu.rs` page tables is a later, separate increment once this one holds up.

**Real risk being tested:** whether "one source, two projections" is actually simpler/safer than "two structures kept in sync" for the specific shapes x86_64 4-level paging and VT-d context/PASID tables need — not assumed, checked.

(Updated as work lands below.)
