# Agentic OS — Build Roadmap

**Status:** Phases 0–9 complete and evidenced (see `docs/PROGRESS.md`). ADR-007 and ADR-008 (§2) signed off 2026-09-03 — Phases 11 and 12 are unblocked. Phase 9 (SMP) completed 2026-09-04. **Phase 9.5 (self-healing and self-improvement) added 2026-09-04 and sequenced before Phase 10** — found while planning Phase 10, whose own deliverable 4 depends on real crash-and-restart machinery that does not yet exist; **gated on ADR-009 and ADR-010 sign-off, both currently Proposed.** Phase 11 (Tier 3 hardware breadth) is unblocked and may proceed in parallel; Phase 12 (display server) still additionally depends on Phase 3's GPU path.
**Date:** 2026-08-27; Phases 9–15 added 2026-09-03
**Supersedes:** the previous `docs/ROADMAP.md` (preserved in git history at commit `6818872`)
**Scope:** ground-up construction plan derived from the code actually in this repository, extended from "Tier 1/Tier 2 code-complete" (original Phase 8 endpoint) through to "a real, daily-driveable OS" (Phase 15)

---

## 1. Constraints And Definitions

### 1.1 The hard constraint

Agentic OS does not run on, inside, or on top of Linux, Windows, or any other host kernel at runtime. The artifact is a bootable image that owns the hardware. No WSL, no host syscall passthrough, no hypervisor-guest dependency on a host OS's services.

This constraint is architectural, not stylistic. It exists because the product thesis — an OS whose primitives are designed for agents rather than for humans driving apps — cannot be validated on a substrate whose primitives were designed for the opposite. Building on Linux would mean inheriting POSIX file descriptors, uid/gid permissions, and signal semantics, which is precisely the "jam on bread" outcome being rejected.

### 1.2 What "from scratch" does and does not mean

Confusing "we wrote it" with "we own it" produces bad decisions in both directions. The line:

| Category | Verdict | Reasoning |
|---|---|---|
| Cross-compiler (GCC/Clang/LLVM) | **Permitted** | Every OS in existence is built with a toolchain someone else wrote. Writing a compiler is a different project. |
| CPU/firmware specifications (UEFI, ACPI, PCIe, VirtIO, xHCI) | **Permitted and required** | Implementing a public specification yourself *is* from-scratch. Reading the spec is not borrowing. |
| Build/test host environment (Docker, QEMU) | **Permitted** | These are development tools, not runtime dependencies. The shipped image contains none of them. |
| `gnu-efi` (currently used in `boot/main.c`) | **Permitted now, retire in P0** | It is a thin convenience wrapper over UEFI calls. Keeping it is fine; owning it removes an external dependency from the boot path. Low priority. |
| Reading Linux/BSD driver source as reference | **Permitted with care** | Reading to understand hardware behavior is fine. Copying GPL code into this kernel imposes GPL on the kernel. Implement from spec; use existing drivers only to resolve undocumented behavior, and note it. |
| Porting musl / newlib / any existing libc | **Rejected** — see ADR-004 | The syscall ABI is capability-based, not POSIX. A POSIX libc has nothing to bind to. |
| Linux kernel, driver framework, or userland as a runtime base | **Rejected** | Violates §1.1. |

### 1.3 Non-goals, stated up front

These are not deferred. They are out of scope for the foreseeable roadmap, and any work that assumes them should be rejected in review:

- Broad hardware compatibility across arbitrary consumer laptops and desktops.
- Binary compatibility with Linux, Windows, or POSIX applications.
- A desktop environment, window manager, or graphical application framework.
- Multi-architecture support. `x86_64` only until the entire roadmap below is complete.
- Any model inference, prompt handling, or agent reasoning inside the kernel. Ever.

#### 1.3.1 Two of these non-goals are deliberately reversed starting at Phase 11 — with their own sign-off, not silently

"Real, daily-driveable OS" (Phases 9–15, added below) genuinely conflicts with two of the five bullets above: hardware breadth and a graphical surface. Rather than quietly drop them or pretend Phase 8's scope always included them, the reversal is named explicitly, each with its own ADR (ADR-007, ADR-008 in §2), because that is what this document's own review discipline (§8) requires of any change this consequential.

**What stays non-negotiable, unchanged, forever:** binary compatibility with Linux/Windows/POSIX applications is still rejected — a GUI and broader hardware support are built on the same capability ABI (ADR-003/004), never a POSIX compatibility shim. Multi-architecture support and in-kernel model inference remain out of scope through the entire roadmap, Phase 15 included.

**What changes and why:** a real OS people can point at next to Linux, macOS, or Windows needs to run on more than one specific laptop, and needs a visual surface — refusing both forever would cap this project at "an impressive research kernel," not the stated destination. Phase 11 (§5) reverses the hardware-breadth non-goal, narrowly — a validated matrix of several real machines, not "arbitrary consumer hardware," and every addition still goes through the Phase 6 driver-synthesis loop's containment guarantees, never a carve-out. Phase 12 reverses the GUI non-goal, but as a genuinely agent-native visual surface (every window and widget a typed, capability-scoped, introspectable object per Phase 5's own discipline extended to pixels) rather than a conventional desktop environment ported in from outside that model.

---

## 2. Foundational Architecture Decisions

These five decisions constrain every phase that follows. They are placed first because retrofitting any one of them costs more than the entire phase it would have shaped. **Each requires explicit sign-off before Phase 0 starts.**

---

### ADR-001: Microkernel-leaning architecture with user-space drivers

**Status:** Proposed
**Context:** The current kernel is monolithic — `keyboard.c`, `graphics.c`, and `serial.c` all execute in ring 0 with unrestricted memory access. The product vision includes agents that synthesize and install drivers. These two facts are incompatible.

A driver written by an agent will, at some point, be wrong. In a monolithic kernel, a wrong driver corrupts kernel memory, hangs the machine, or silently destroys data — and there is no error to feed back into a retry loop, because the machine is gone. The self-healing driver loop is only implementable if a driver failure is a *recoverable, observable event*.

#### Options Considered

**Option A: Monolithic kernel (continue current trajectory)**

| Dimension | Assessment |
|---|---|
| Complexity | Low — no IPC layer needed |
| Performance | Best — direct function calls, no context switches |
| Agent-driver safety | **Unacceptable** — a bad driver is an unrecoverable system failure |
| Fault isolation | None between subsystems |

**Pros:** Simplest path; fastest raw I/O; matches existing code.
**Cons:** Makes the central product differentiator (agent-synthesized drivers) unsafe to build. A driver bug is indistinguishable from a kernel bug.

**Option B: Pure microkernel (seL4-style — memory, scheduling, IPC only)**

| Dimension | Assessment |
|---|---|
| Complexity | High — everything becomes an IPC protocol |
| Performance | Weakest — IPC on every operation |
| Agent-driver safety | Strongest |
| Fault isolation | Complete |

**Pros:** Maximum isolation; smallest trusted computing base; formally analyzable.
**Cons:** Every subsystem needs a protocol design before it can be written. Substantially slows Phases 1–4. Risk of the project stalling in protocol design.

**Option C: Microkernel-leaning hybrid — kernel retains memory, scheduling, IPC, capabilities, interrupt dispatch; drivers, filesystems, and network stack run in user space**

| Dimension | Assessment |
|---|---|
| Complexity | Medium-high |
| Performance | Adequate — IPC cost confined to device I/O paths |
| Agent-driver safety | Strong — driver fault kills a process, not the system |
| Fault isolation | Strong at the boundary that matters |

**Pros:** A crashed driver is a restartable process and a log entry — exactly the feedback signal the driver-synthesis loop requires. Keeps scheduling and memory fast paths in-kernel. Trusted computing base stays small enough to reason about.
**Cons:** Requires a working IPC layer before any driver work. Interrupt delivery to user space is non-trivial. Slower I/O than monolithic.

#### Decision

**Option C.** The kernel's permanent residents are: physical and virtual memory management, thread scheduling, address spaces, capability enforcement, IPC, interrupt dispatch to user-space handlers, and the audit log. Everything else — every driver, every filesystem, the network stack, the agent runtime — is a user-space process.

#### Trade-off Analysis

The performance cost is real and it is accepted. A user-space `virtio-net` driver will be measurably slower than an in-kernel one. That cost buys the single property the product depends on: *a driver failure is a contained, observable, retryable event.* Without it, the driver-synthesis milestone cannot be built safely on real hardware at any point, ever. Choosing monolithic for speed would mean the flagship capability is permanently blocked on a rewrite.

#### Consequences

- **Easier:** driver failure recovery; agent-generated code containment; independent driver restart; reasoning about the trusted computing base.
- **Harder:** I/O throughput; interrupt latency; every driver needs a protocol definition; debugging spans process boundaries.
- **Revisit when:** measured I/O throughput blocks a concrete requirement. Selectively moving one hot-path driver in-kernel is a legitimate future optimization — but only for drivers written and reviewed by humans, never agent-synthesized ones.

---

### ADR-002: Implementation language for new kernel subsystems

**Status:** Proposed — **this is the decision most likely to need your override**
**Context:** 1,817 lines of freestanding C exist. The codebase is small enough that a language change is still cheap; in six months it will not be.

#### Options Considered

**Option A: Continue in C**

| Dimension | Assessment |
|---|---|
| Complexity | Low — toolchain works, code exists |
| Migration cost | Zero |
| Memory safety | None — every bug class remains available |
| Ecosystem for OS dev | Mature, universal reference material |

**Pros:** No rewrite. Every OS tutorial, spec example, and reference driver is in C. Existing bootloader and kernel keep working. Agents generate kernel-grade C reliably because the training corpus is enormous.
**Cons:** The project's thesis is a *trustworthy substrate*. In C, that trustworthiness must come entirely from discipline and testing. Use-after-free and buffer overruns in a page-table walker are exactly the bug class that is hardest to find and most catastrophic.

**Option B: Rewrite in Rust (`no_std`)**

| Dimension | Assessment |
|---|---|
| Complexity | Medium-high — borrow checker in kernel context is genuinely difficult |
| Migration cost | ~1,800 lines rewritten; toolchain rebuilt |
| Memory safety | Enforced outside `unsafe` blocks |
| Ecosystem for OS dev | Good and improving (`x86_64` crate, Redox precedent) |

**Pros:** Memory safety directly serves the product thesis. `unsafe` blocks become an auditable, greppable inventory of exactly where the guarantees stop. Concurrency safety matters enormously once the scheduler exists.
**Cons:** Real rewrite cost. Kernel Rust involves substantial `unsafe` regardless, so safety is not automatic. Steeper debugging. Smaller reference corpus for x86_64 bring-up specifics.

**Option C: Hybrid — keep C boot path, write new subsystems in Rust with an FFI boundary**

| Dimension | Assessment |
|---|---|
| Complexity | High — two toolchains, two idioms, FFI at every seam |
| Migration cost | Low initially, compounding later |
| Memory safety | Partial, and weakest exactly at the FFI seams |
| Ecosystem | Both, plus the friction between them |

**Pros:** No upfront rewrite; incremental adoption.
**Cons:** In a 1,800-line codebase this buys little and costs a permanent seam. FFI boundaries are where safety guarantees are laundered away. Two build systems for one kernel.

#### Decision

**Recommend Option B, with a caveat that makes Option A fully defensible.**

The recommendation is Rust because the codebase is currently at its smallest — the rewrite will never again be this cheap — and because "trustworthy substrate for autonomous agents" is a claim that memory safety directly supports and that discipline alone supports only weakly.

The caveat: this is a velocity-versus-safety trade, and **velocity here depends on how reliably your agent fleet produces correct code in each language.** If agent-generated kernel Rust in your hands produces more churn than agent-generated kernel C, Option A is the correct choice and the safety gap should be closed with aggressive host-side unit testing (Phase 0) and fault injection instead. Do not adopt Rust on principle if it halves throughput.

Option C is not recommended at this size.

#### Consequences

- **If Rust:** Phase 0 absorbs the rewrite. Every phase after inherits safety guarantees. `unsafe` block count becomes a tracked project metric.
- **If C:** Phase 0 must add host-side unit tests and sanitizer builds for all memory-management code as compensating controls; these become non-optional rather than nice-to-have.
- **Revisit:** never, in practice. This decision is effectively permanent after Phase 1.

---

### ADR-003: Capability-based security in the first syscall, not a later layer

**Status:** Proposed
**Context:** The previous roadmap placed the "agentic layer" at milestones M10–M11 — last. That ordering is architecturally unsound. Capability-based security is a property of the syscall ABI itself. A POSIX-style ABI (integer file descriptors, ambient process authority, uid/gid checks) cannot be converted into a capability system by adding a layer above it, because ambient authority is defined by what the *lower* layer permits. Retrofitting means rewriting every syscall and every caller.

**Decision:** The first syscall this OS ever implements is capability-based. There is no interim POSIX-shaped ABI.

Concretely, from Phase 2 onward:

- A process holds an explicit capability table. It cannot name a resource it has not been granted.
- There is no ambient authority. No `open("/etc/passwd")` that succeeds because the caller happens to be privileged — the caller either holds a capability to that object or the call fails.
- Capabilities are unforgeable, explicitly delegated, attenuable (a process can pass a weaker derivative of a capability it holds, never a stronger one), and revocable.
- Every capability invocation writes an audit record (ADR-005).

**Consequences:**

- **Easier:** sandboxing an agent becomes the default state rather than an added restriction; least-privilege is structural; auditability is complete by construction.
- **Harder:** no POSIX software will ever run unmodified; every service needs explicit capability plumbing; the libc must be written for this ABI (ADR-004).
- **This is the decision that makes the OS agent-native.** Without it, the product is a conventional kernel with an agent process on top — the outcome §1.1 exists to prevent.

---

### ADR-004: Purpose-built C library, not a port

**Status:** Proposed
**Context:** ADR-003 makes the syscall ABI non-POSIX. musl and newlib exist to implement POSIX semantics over a POSIX kernel interface. There is no POSIX kernel interface here for them to bind to. Porting one would mean writing a POSIX emulation layer over the capability ABI — which reintroduces ambient authority, defeating ADR-003.

**Decision:** Write a minimal freestanding library exposing the capability ABI directly. It provides memory and string primitives, formatted output, and typed capability-invocation wrappers. It does not provide `fopen`, `fork`, `signal`, or anything else whose semantics assume ambient authority.

**Consequences:** No third-party C software ports without modification — an accepted cost, consistent with §1.3. The library stays small enough to audit.

---

### ADR-005: The audit log is a kernel primitive, not a service

**Status:** Proposed
**Context:** If the audit log is a user-space service that processes voluntarily call, an agent that misbehaves is exactly the process least likely to call it. Auditability that depends on the audited party's cooperation is not auditability.

**Decision:** Capability invocation and audit-record emission are the same kernel code path. A capability cannot be exercised without producing a record. Records carry: invoking process identity, capability identity, operation, timestamp, and result. The log is append-only from user space; no capability grants the right to rewrite or delete history.

**Consequences:**

- **Easier:** complete, trustworthy provenance for every agent action; a real forensic trail when a driver-synthesis attempt corrupts something.
- **Harder:** every syscall pays a logging cost; log storage and rotation become a Phase 4 requirement; the kernel grows a subsystem that is arguably policy.
- **Revisit:** if measured syscall overhead becomes a blocker, the record *format* can be optimized. The invariant that invocation implies a record cannot be relaxed.

---

### ADR-006: IOMMU is mandatory before any agent-generated driver touches hardware

**Status:** Proposed
**Context:** This is the failure mode most often missed. Page tables constrain what a *CPU* can access. They do not constrain **DMA**. A device programmed with a bad descriptor address will write into arbitrary physical memory regardless of how carefully the driver process is sandboxed by the MMU. A user-space driver without IOMMU protection is not meaningfully isolated — it is a process that can corrupt any physical page by asking a device to do it.

**Decision:** Intel VT-d / AMD-Vi support, with per-device DMA domains, is a hard prerequisite for Phase 6. A driver process gets a DMA domain covering exactly its own buffers. Any agent-synthesized driver that runs before this exists — even in a VM — is running without the isolation the design claims.

**Consequences:** Phase 3 must include IOMMU bring-up. Hardware without an IOMMU is out of scope for agent-synthesized drivers, permanently. This is a real hardware requirement to state before choosing the Tier 2 physical machine (§4).

---

### ADR-007: Hardware breadth is grown through Tier 3, never opened wide

**Status:** **Signed off 2026-09-03.** Option C adopted as written.
**Context:** §1.3 rejects "broad hardware compatibility across arbitrary consumer laptops and desktops," correctly, for Phases 0–8: chasing breadth before the substrate is trustworthy is how from-scratch OS projects die (§4's own words). But a real OS that only ever boots one specific ThinkPad is not a real OS — it is a proof of concept with a hardware dependency. The tension is real and both sides of it are correct at different points in the roadmap.

#### Options Considered

**Option A: Never revisit §1.3 — stay Tier 1 + one Tier 2 machine forever.**
Pros: zero risk of the breadth trap this project explicitly rejected. Cons: permanently caps the project below its own stated destination ("a real bootable OS like Linux, Mac, Windows"); Tier 2's single-machine validation is not evidence the OS works on hardware in general, only on that hardware.

**Option B: Open hardware support immediately and broadly once Phase 8 closes.**
Pros: fastest path to breadth. Cons: exactly the failure mode §4 warns about — driver count explodes faster than the synthesis loop's (Phase 6) containment guarantees can be re-validated per device, and "supported" quietly comes to mean "compiles," not "verified."

**Option C: Grow a bounded, explicitly tracked Tier 3 matrix (a handful of real, named machines spanning at least two chipset generations and both a laptop and a desktop class), every addition going through the same Phase 6 synthesis-and-containment discipline already proven at Tier 2, with the matrix published and narrow rather than implied to be universal.**
Pros: real breadth, without abandoning the discipline that got this project this far; "supported" keeps meaning what `docs/SUPPORTED_HARDWARE.md` has meant since Phase 8 — narrow, honest, evidenced. Cons: slower than Option B; the matrix will always be smaller than Linux's, and that must stay stated, not hidden.

#### Decision

**Option C.** Tier 3 is a small, named, published list — not an open compatibility claim. Every device added to it is added the same way NVMe/AHCI/e1000 were added to Tier 2: spec-driven or synthesis-loop-driven, IOMMU-contained, evidenced.

#### Consequences

- **Easier:** a real, defensible "runs on N real machines" claim; the driver-synthesis loop (Phase 6) gets real, repeated exercise instead of staying a one-shot demonstration.
- **Harder:** every new machine is real work, not a checkbox; the matrix must be maintained or it silently rots into a false claim, which §8's review discipline treats as seriously as any other unevidenced claim.
- **Revisit:** if the synthesis loop's failure rate on genuinely novel hardware (not just genuinely novel devices on known chipsets) turns out too high to sustain Option C's pace, that is itself a finding worth a written retrospective, not silent breadth-capping.

---

### ADR-008: A visual surface is capability-native, not a ported desktop environment

**Status:** **Signed off 2026-09-03.** Option C adopted as written.
**Context:** §1.3 rejects "a desktop environment, window manager, or graphical application framework" for the same reason it rejects POSIX compatibility — those are designed for ambient-authority, human-driven systems, and porting the *shape* of one (even with a from-scratch implementation) would reintroduce exactly what ADR-003 exists to prevent: a window that can read another window's buffer because it happens to share a display, the GUI equivalent of ambient authority.

#### Options Considered

**Option A: No GUI, ever — stay serial-shell-only.**
Pros: avoids the risk entirely. Cons: "a real OS like Linux, Mac, Windows" without a display server is not a credible claim; it caps the project below the destination the user has now explicitly set.

**Option B: Port or closely imitate a conventional compositor/window-manager architecture (a `Wayland`-shaped or `X11`-shaped design), implemented from scratch to satisfy §1.2's "from scratch" bar.**
Pros: mature, well-understood architecture; lots of reference material. Cons: those architectures assume a trusted compositor mediating cooperating clients under a shared-desktop threat model — not the "an agent process is adversarial until proven otherwise" threat model this OS has held since Phase 5. It would be the first subsystem in the whole roadmap built to a different security model than everything around it.

**Option C: A compositor that is itself an ordinary capability-holding user-space service (consistent with ADR-001), where every window, surface, and input event is a typed, capability-scoped, audit-logged object — extending Phase 5's introspection API to pixels and input, not bolting a UI on beside it.**
Pros: a misbehaving window is contained exactly like a misbehaving driver or agent already is; an agent manipulates the UI through the same typed interface Phase 5 already requires for everything else — no pixel-scraping, ever, which is also just a straightforwardly better design for an agent-native OS. Cons: real, novel design work — there is no reference implementation of a capability-native compositor to crib from; slower than B.

#### Decision

**Option C.** The display server is a service like any other in this architecture, not a special case. "No pixel-scraping" is a hard requirement carried over from Phase 5's ADR-implied discipline, not a Phase 12 invention.

#### Consequences

- **Easier:** GUI security inherits everything Phases 2–5 already built, rather than needing its own parallel security model; an agent driving the UI is exercising the exact same typed-invocation path a human-typed shell command does (Phase 7's own equivalence requirement extends naturally to the GUI).
- **Harder:** no shortcut through an existing compositor design; input routing, damage tracking, and GPU buffer sharing all need capability-scoped designs from zero.
- **Revisit:** never, in practice, for the same reason ADR-002 is effectively permanent after Phase 1 — the security model a GUI is built on cannot be swapped out once applications exist that depend on it (Phase 13).

**Consequences of both ADR-007 and ADR-008 together:** Phases 9–10 (SMP, networking) do not depend on either and can proceed under unchanged rules. Phases 11 and 12 are the first work in this entire roadmap that requires revisiting a signed-off constraint from §1 — treat their sign-off with the same weight §2's original five decisions carried, not as a routine phase-start.

---

### ADR-009: Self-modification is evidence-gated adoption — the OS is the judge, never the author

**Status:** Proposed — **requires sign-off before Phase 9.5 starts.**

**Context:** The stated goal is an OS that heals itself, upgrades its own code, fixes its own bugs, and adopts research breakthroughs into the running system. That goal collides head-on with §1.3's hardest non-goal — "any model inference, prompt handling, or agent reasoning inside the kernel. Ever." — which §1.3.1 explicitly declined to reverse even while reversing two others ("in-kernel model inference remain[s] out of scope through the entire roadmap, Phase 15 included").

The collision is only apparent, and resolving it correctly is the whole point of this ADR. "The OS improves itself" contains two entirely separable capabilities: **authoring** a change, and **deciding whether that change may take effect**. Only the first requires reasoning. The second requires containment, verification, evidence, and rollback — which is precisely what this OS has spent nine phases building, and precisely what a language model is worst at being trusted with.

There is also direct precedent inside this project. Phase 6 already built this exact loop for one narrow case: a synthesized `virtio-net` driver, run against a disposable snapshot, classified by real captured failure evidence, retried under a bounded budget, promoted only on a real success marker. Two facts about it matter here. First, it works, and it caught a real bug in a real synthesized driver (the 10-byte vs. 12-byte `virtio_net_hdr`) using independent host-side evidence rather than the component's own self-report. Second, **the loop lives in `scripts/test-synthesis.ps1` — on the host, outside the OS.** It is a developer's test harness, not a capability the running system possesses. Phase 9.5 is the work of moving that judgment inside, and generalizing it from drivers to the system.

Finally: `docs/PRIOR_ART.md`'s own honest verdict already identified "hardware-contained evidence-gated driver-code promotion" as this project's **most defensible novelty** — the one item in a long table of "not novel" findings that survived a real prior-art review. This ADR generalizes exactly that mechanism. That is not a coincidence to gloss over; it is the strongest available signal that this is the right thing to build.

#### Options Considered

**Option A: The OS authors its own improvements — inference inside the kernel, or inside a trusted kernel-adjacent service.**
Pros: the most literal reading of "the OS improves itself"; no external dependency at improvement time. Cons: directly violates §1.3, the one non-goal this roadmap has never softened. Worse, it is bad design independent of that rule: the component proposing a change would also be the component judging it, which removes the only independent check in the system. A model confidently wrong about its own output is the normal case, not the edge case — and here the blast radius is the kernel. Rejected on both grounds.

**Option B: Keep the loop host-side, as Phase 6 built it.**
Pros: already works, already evidenced, zero new risk. Cons: does not meet the goal at all. A PowerShell script that rebuilds and reboots a kernel is a build system; calling it self-improvement would be exactly the kind of unevidenced claim §8's review discipline exists to reject.

**Option C: The OS owns containment, verification, promotion, rollback, and audit. The proposer is external and untrusted — an agent, a human, or a research process — and its output is treated exactly like a synthesized driver binary is treated today: as an input to be judged, never as an authority to be obeyed.**
Pros: satisfies the goal (the running system genuinely changes its own code, adds features, fixes its own bugs, adopts research results) while keeping every line of reasoning outside the kernel, so §1.3 holds unmodified. The OS's contribution is the part that is actually hard and actually novel — deciding, on real evidence, whether a change is allowed to become part of itself. Cons: the OS cannot originate a breakthrough on its own; it can only evaluate and adopt one. That limitation is real and must be stated plainly wherever this capability is described, rather than allowed to blur into a stronger claim.

#### Decision

**Option C.** With four binding clauses, each of which exists to make "the system improved itself" a *falsifiable* statement rather than a story told afterward:

1. **The proposal is untrusted input.** A proposed change — new feature, bug fix, or research-derived improvement — enters the system as data, with no privilege whatsoever, exactly like Phase 6's synthesized driver binary. Its origin (which agent, which model, which human) is recorded but confers no trust.

2. **The acceptance criterion is declared *before* the trial, and is machine-checkable.** A change is promoted if and only if a predicate fixed in advance passes against real captured evidence — real test-suite results, real absence of new exceptions, real absence of new IOMMU faults, real performance within a stated bound. Criteria invented or adjusted after seeing the outcome are forbidden. This single rule is what separates this from "we ran it and it seemed fine."

3. **Adoption is A/B with a always-bootable predecessor, never in-place mutation.** The trial runs contained; on pass, the new version is promoted; on fail, it is discarded. The previous known-good version remains bootable at every instant, using the same durability discipline Phase 8's `test-powerloss.ps1` already proves. A rollback must leave the system byte-identical to its pre-trial state, and that is an exit criterion, not an aspiration.

4. **Bounded attempts, then a human.** Mirroring Phase 6 deliverable 3: a fixed retry budget, after which the system stops, keeps the last known-good version running, and hands over the complete attempt history. An unbounded self-modification loop is how this capability turns into the failure mode it exists to prevent.

And **three tiers, separated by how much is genuinely the OS's own work**, because collapsing them is how this claim would get overstated:

- **Tier A — policy self-tuning (no code change at all).** The system adapts its own parameters from its own recorded outcomes: restart backoff derived from real crash history rather than a hardcoded constant; least-privilege authority envelopes narrowed from real observed IOMMU faults (promoting research §3 out of `docs/RESEARCH_TRACK.md` into production). Fully local, fully verifiable, no external proposer needed. This is the only tier the OS does entirely by itself, and it should be described that way.
- **Tier B — contained code adoption.** An externally-proposed code change is built, trialled under containment, judged against its pre-declared criterion, and promoted or rejected with full audit and rollback. This is the substantive new engineering.
- **Tier C — research-originated adoption.** Identical machinery to Tier B, with one addition: a proposal claimed as a breakthrough must additionally pass a prior-art gate before that claim may be recorded, held to the standard `docs/PRIOR_ART.md` already established. The OS evaluates and adopts research results; it does not perform the research.

#### Consequences

- **Easier:** the capability the project's own prior-art review identified as its most defensible novelty stops being driver-specific and becomes a system property. Phase 6's synthesis loop, Phase 3's restart-on-crash, Phase 2's audit log, and ADR-006's IOMMU containment all turn out to be components of this one mechanism rather than separate features — which is a strong sign the architecture was right.
- **Harder:** every self-modification path needs a durable, verifiable rollback story before it may ship, and "verified" now means "passed a criterion someone committed to in advance," which is a materially higher bar than the project has held itself to anywhere except Phase 6.
- **The honest limit, stated here so it can be quoted rather than rediscovered:** passing a declared criterion is not proof of correctness. It is proof that the change did not violate the specific properties someone thought to check. An under-specified criterion produces confident adoption of a bad change, and the system has no way to know that happened. This is the single largest risk in the phase and must be named in any external description of it.
- **Second honest limit:** a change verified on QEMU is verified on QEMU. Until the Tier 2 physical-hardware gap (Phase 8) closes, self-adopted changes carry exactly the same hardware-validation caveat every other part of this system does.
- **Revisit:** if Tier B's promote/reject decisions turn out to be dominated by criteria that are trivially satisfiable, the mechanism is providing false assurance and is worse than not having it — that finding warrants a written retrospective and a halt, not a criterion tweak.

---

### ADR-010: A permanently immutable core, excluded from self-modification

**Status:** Proposed — **requires sign-off before Phase 9.5 starts.**

**Context:** ADR-009 lets the system adopt changes to its own code on evidence. That machinery contains an obvious and fatal circularity if left unbounded: a change to the verification harness can make every subsequent change pass. A change to the audit log can erase the record of what happened. A change to the capability enforcement core can grant the proposer whatever it likes. A change to the rollback mechanism can make the escape hatch stop working. In each case the system would continue reporting success, truthfully by its own standards, while its actual safety properties were gone.

This is not hypothetical reasoning about adversarial agents; it is the ordinary consequence of a bug landing in the wrong file. Phase 6 already set the precedent for handling exactly this class of problem: its deliverable 5 permanently excludes device classes whose failure modes are irreversible — firmware flashing, embedded controller commands — with the word "permanently," not "for now."

#### Options Considered

**Option A: Nothing is immutable — any component may be self-modified if it passes its criterion.**
Pros: maximal capability; no special cases to maintain. Cons: the circularity above, in full. The first change that weakens the judge invalidates every judgment after it, including the ones that would have caught the problem. Rejected.

**Option B: Everything requires human sign-off before promotion.**
Pros: no circularity possible; a human is always in the loop. Cons: this is not self-improvement, it is a code review workflow with extra steps. It also does not actually solve the problem it appears to — a human approving a diff to the verification harness under time pressure is precisely how this failure mode occurs in real systems.

**Option C: A small, explicitly enumerated, permanently immutable core that self-modification may never touch, enforced mechanically and audited on every refusal; everything outside it is eligible under ADR-009's normal evidence gate.**
Pros: preserves the capability where it is valuable while making the circularity structurally impossible rather than merely discouraged; the boundary is small enough to review by hand and stated explicitly enough to test against. Cons: the core can only be changed by a human editing it and rebuilding through the normal development path — which is the intended cost, not a defect.

#### Decision

**Option C.** The immutable core is, at minimum:

1. **The capability enforcement path** — `capability.rs`'s resolve/derive/revoke logic. Everything in ADR-003 rests on it.
2. **The IOMMU containment path** — `iommu.rs`'s domain assignment and invalidation. ADR-006 is unconditional; a self-modifiable containment layer is not containment.
3. **The audit log** — ADR-005 already makes it a kernel primitive rather than a service, for closely related reasons. A self-modifiable record of what happened is not a record.
4. **The verification and promotion machinery itself** — the criterion evaluator, the trial harness, and the promote/reject decision.
5. **The rollback mechanism and the known-good version pointer** — the escape hatch may never be modified by the thing it exists to escape from.

Two binding rules: the list may only be *extended* by a human editing this ADR, never narrowed by a running system; and an attempted self-modification touching anything on it is **refused and audited**, never silently dropped — a refusal that leaves no evidence is indistinguishable from an exploit that succeeded.

#### Consequences

- **Easier:** the safety argument for ADR-009 becomes checkable by inspection — the question "could a bad change disable the thing that would catch it?" has a bounded, enumerable answer instead of requiring whole-system reasoning.
- **Harder:** the boundary must be enforced mechanically and tested adversarially (a proposal deliberately targeting the core, refused and audited, is a Phase 9.5 exit criterion), and it will occasionally block a change that would genuinely have been an improvement. That cost is accepted.
- **Revisit:** only to *extend* the list. If experience shows some component outside it can also invalidate future judgments, it belongs inside it, and that is a documentation change plus an enforcement change, never a runtime decision.

---

## 3. Current State — Grounded Assessment

Every claim below is traceable to a file in this repository. This section exists so that phase planning starts from what is true rather than what was intended.

### What works

| Subsystem | Location | State |
|---|---|---|
| UEFI bootloader | `boot/main.c` | ELF64 validation, exact-address `PT_LOAD` mapping, GOP framebuffer discovery, memory-map/ExitBootServices retry loop, explicit failure paths for ~15 error conditions |
| Boot handoff | `include/bootinfo.h` | Versioned `BootInfo` with magic/version validated at `kernel/kernel.c:15` |
| Serial logging | `kernel/klog.c`, `kernel/serial.c` | Hand-rolled `printf` formatter over raw UART, no libc dependency; initialized before all other subsystems |
| Physical memory | `kernel/memory.c` | Two-bitmap PMM (used + reserved), eight explicitly reserved regions, alloc/free with rejection of unaligned, out-of-range, reserved, and double frees |
| Segmentation | `kernel/gdt.c` | Null + kernel code + kernel data descriptors, far-return segment reload |
| Interrupt entry | `kernel/idt.c`, `kernel/pic.c` | 256-entry IDT, 8259 remapped to 0x20/0x28 |
| Framebuffer output | `kernel/graphics.c`, `kernel/print.c` | Double-buffered rect fill and PSF1 glyph rendering |
| Build and boot evidence | `Makefile`, `_evidence/` | Docker build to bootable FAT image; headless QEMU boot asserting ordered serial checkpoints |

### What is structurally blocking

These are not "incomplete features." They are properties of the current code that prevent the phases below from being built on top of it.

1. **The kernel is not higher-half.** `kernel/linker.ld:5` links at `0x200000`. `kernel/paging.c:77-86` identity-maps every firmware-reported region. There is no user/kernel address split, so there is nowhere to put a user address space. **This blocks all of Phase 1.**

2. **Every page is mapped `PAGE_PRESENT | PAGE_READ_WRITE`** (`kernel/paging.c:53`). No NX, no read-only text, no user/supervisor distinction. There is no `unmap`, and no TLB invalidation anywhere in the file. Memory protection is currently *absent*, not partial.

3. **Page zero is PMM-reserved but VMM-mapped.** `kernel/memory.c:129` reserves it from allocation; `paging.c:77-86` maps it anyway if firmware reports it. Null-pointer dereference does not fault.

4. **Three of thirty-two CPU exceptions are handled** (`kernel/interrupts.c:51-53` — #DE, #GP, #PF). There is no TSS and no IST, so a fault during fault handling is undefined behavior rather than a diagnosable panic. Debugging everything downstream depends on fixing this first.

5. **The keyboard ISR does unbounded work in interrupt context.** `kernel/keyboard.c:31` calls `swap_buffers()`, which is a full-framebuffer `memcpy` (`kernel/graphics.c:51-57`) — multiple megabytes copied per keystroke, inside an interrupt handler, with a scalar loop.

6. **No timer.** The PIC is remapped but only IRQ1 is unmasked (`kernel/interrupts.c:62`). Without a periodic timer there is no preemption, therefore no scheduler.

7. **No heap.** No allocator exists in the tree. `vmm_alloc_pages` (`kernel/paging.c:56`) hands out page-granular virtual memory from a monotonically increasing counter with no free path.

8. **`make test-host` is a stub** that prints "No host unit tests exist yet." The PMM bitmap logic is pure, deterministic, and trivially testable on the host — and is currently untested.

9. **Stray artifacts in the repo root.** `test.c`, `test.ld`, `test.elf`, `test.o` are an unrelated ELF-loader stub, not a test harness. Delete in Phase 0.

---

## 4. Hardware Targets

Chasing hardware breadth is how from-scratch OS projects die. Two tiers, explicitly:

**Tier 1 — QEMU `q35` + OVMF + VirtIO.** The development and CI target. Every phase gate is validated here. VirtIO devices are fully specified, spec-compliant, and safe to iterate against. All driver-synthesis work begins here.

**Tier 2 — one specific physical machine, chosen at Phase 3.** Selection criteria, in order: (a) IOMMU present and functional (ADR-006 makes this non-negotiable), (b) serial output reachable — physical port or USB-serial with a working early-boot path, (c) NVMe or AHCI storage, (d) publicly documented chipset. A machine that cannot emit serial output early in boot is unusable for this project regardless of its other merits.

Everything else is out of scope per §1.3.

---

## 4.5. Research Track (Parallel To Phases, Never Blocking Them)

Separate from the numbered phases below: `docs/NOVEL_CONCEPTS.md` proposes concepts aimed at genuine field-level novelty (not just new-to-this-codebase engineering), gated on the real prior-art review in `docs/PRIOR_ART.md`. Tracked in `docs/RESEARCH_TRACK.md`, built and verified in isolation (`host_tests`, zero boot-path wiring) per this project's standing discipline for unproven ideas -- same path `kernel_common::driver_registry` took. Never inserted into the phase numbering below, and never a dependency of it: the phases are evidence-gated infrastructure the OS needs regardless of whether any research-track concept succeeds. A concept promoted out of the research track becomes a normal, numbered deliverable at that time, decided then -- not reserved for in advance.

---

## 5. Phases

Each phase states its dependency, its deliverables, the exit criteria that must be *demonstrated*, and the evidence artifact that demonstrates them. A phase is not complete because the code exists; it is complete when the evidence exists.

The evidence discipline already in the repository (`_evidence/latest/`, ordered serial checkpoints, `make test-boot`) carries forward unchanged — it is the one part of the current process that needs no redesign.

---

### Phase 0 — Foundation Correctness

**Depends on:** ADR sign-off. **Blocks:** everything.

This phase adds no features. It repairs the structural defects in §3 that make later phases unbuildable. Every item traces to a numbered defect above.

**Deliverables**

1. Language decision from ADR-002 executed. If Rust: the existing kernel is ported, and the bootloader either ports or keeps a documented C boundary. If C: host-side unit tests and sanitizer builds for all memory code become mandatory, as the compensating control.
2. All 32 CPU exception vectors handled with register and fault-address capture. TSS installed; IST stack dedicated to the double-fault path. *(Defect 4)*
3. VMM redesigned:
   - Higher-half kernel — new `linker.ld` base, new virtual layout: null guard unmapped, higher-half kernel text/data, physical direct-map window, MMIO window, kernel heap window, user region below the split. *(Defects 1, 3)*
   - Per-page permission bits: NX on data, read-only on kernel text, user/supervisor distinction. *(Defect 2)*
   - `unmap` implemented; TLB invalidation via `invlpg` on every mapping change. *(Defect 2)*
   - The blanket identity map is deleted. Early low identity mapping exists only during transition and is torn down.
4. Kernel heap: `kmalloc`, `kfree`, `kcalloc`, aligned allocation, with a free path. *(Defect 7)*
5. Local APIC timer as the periodic tick, replacing PIC-only interrupt handling. *(Defect 6)*
6. Deferred interrupt work: IRQ handlers capture event state into a queue and return. All rendering, parsing, and scheduling moves out of interrupt context. `swap_buffers` leaves the keyboard ISR. *(Defect 5)*
7. Host test harness: `make test-host` runs real assertions against PMM bitmap logic, VMM address translation, and heap allocation, compiled for the host. *(Defect 8)*
8. Fault-injection test: `make test-faults` deliberately triggers a null dereference, a write to read-only kernel text, an execute attempt on NX data, and a double fault — asserting each produces the correct diagnosable panic rather than a hang. *(Validates 2, 3, 4)*
9. Repo hygiene: delete `test.c`, `test.ld`, `test.elf`, `test.o`. *(Defect 9)*

**Exit criteria — all must be demonstrated**

- Null dereference faults. Write to kernel text faults. Execute on NX data faults. Double fault produces a panic with register state, not a hang.
- Kernel runs from the higher half; no blanket identity map remains in `paging.c`.
- Timer interrupts fire at a measured, asserted rate.
- No interrupt handler performs unbounded work.
- `make test-host` executes real assertions and fails when the logic is broken.

**Evidence:** `_evidence/latest/serial.log` with checkpoints through `VMM_HIGHERHALF_OK`, `HEAP_INIT_DONE`, `TIMER_TICK_OK`; `host-tests.log` with pass/fail counts; `fault-tests.log` with one panic transcript per injected fault.

---

### Phase 1 — Execution Model

**Depends on:** Phase 0. **Blocks:** Phases 2–8.

**Deliverables**

1. Kernel threads with context switch and a preemptive scheduler driven by the APIC timer.
2. Address spaces: per-process PML4, switched on context switch.
3. Ring 3 execution — TSS `RSP0` configured, user segments in the GDT.
4. `SYSCALL`/`SYSRET` entry path with kernel stack switching and full user-pointer validation on every argument.
5. Process lifecycle: create, exit, reap.

**Exit criteria**

- Two user-space processes run concurrently and are preempted by the timer.
- Process A cannot read or write process B's memory — demonstrated by an attempt that faults.
- A user-space fault terminates that process; the system continues running and logs the termination.
- A syscall with a malicious pointer argument is rejected, not honored.

**Evidence:** serial log showing interleaved execution of two processes, an isolation-violation fault transcript, and a process-termination-with-survival transcript.

---

### Phase 2 — Capability And IPC Substrate

**Depends on:** Phase 1. **Blocks:** Phases 3–8. **This phase is where the OS stops being a conventional kernel.**

Per ADR-003, this comes *before* drivers, filesystems, and services — because all of them are defined in terms of capabilities, and defining them first would mean defining them twice.

**Deliverables**

1. Per-process capability table; unforgeable capability references.
2. Capability operations: grant, derive-with-attenuation, revoke. Derivation can only weaken.
3. Synchronous message-passing IPC between processes, capability-gated.
4. Kernel audit log on the capability-invocation path (ADR-005) — invocation and record emission are one code path.
5. Interrupt forwarding to a registered user-space handler process — the mechanism Phase 3 drivers depend on.
6. The syscall surface: small, typed, and complete enough to express everything a user-space driver or service needs.

**Exit criteria**

- A process cannot name or reach any resource it holds no capability for — demonstrated by a denied attempt.
- An attenuated capability provably cannot be re-strengthened.
- Revocation takes effect immediately, including for already-delegated derivatives.
- Every capability invocation in a recorded session appears in the audit log, with no gaps.
- A user-space process receives and acknowledges a hardware interrupt.

**Evidence:** capability-denial transcript; attenuation and revocation test output; audit log diffed against an independently instrumented syscall trace, demonstrating zero missing records; user-space interrupt delivery log.

---

### Phase 3 — User-Space Driver Framework

**Depends on:** Phase 2. **Blocks:** Phases 4–8.

**Deliverables**

1. Driver process model: MMIO regions and interrupt lines granted as capabilities, nothing ambient.
2. **IOMMU (VT-d / AMD-Vi) bring-up with per-device DMA domains** (ADR-006). A driver's device can DMA only into that driver's own buffers.
3. PCIe enumeration as a user-space service.
4. Device manager: discovery, driver binding, lifecycle, restart-on-crash.
5. `init` and a service manager as the first user-space processes.
6. First user-space drivers, ported off the current in-kernel implementations: serial, framebuffer, PS/2 keyboard.
7. Tier 2 physical machine selected and brought to serial output (§4).

**Exit criteria**

- A driver process is killed mid-operation; the device manager restarts it; the system never faults.
- A driver attempting DMA outside its domain is blocked by the IOMMU and the attempt is logged.
- Console output and keyboard input work with zero driver code in ring 0.
- Tier 2 hardware boots to serial output.

**Evidence:** driver-crash-and-restart transcript; IOMMU violation log; serial log from Tier 2 physical hardware.

---

### Phase 4 — Storage And Filesystem

**Depends on:** Phase 3. **Blocks:** Phases 5–8.

**Deliverables**

1. Block device abstraction; `virtio-blk` user-space driver (Tier 1), NVMe or AHCI (Tier 2).
2. On-disk filesystem. Prefer implementing a documented format (FAT32 for the ESP is already required; ext2 is a reasonable read/write target) over inventing one — the spec is public, implementing it is from-scratch per §1.2, and a debuggable format saves weeks.
3. Object store with capability-scoped naming. There is no global namespace an unprivileged process can walk; a process sees what it holds capabilities to.
4. Audit log persistence, with rotation (deferred obligation from ADR-005).

**Exit criteria**

- Data written survives a reboot and reads back byte-identical.
- A process without a capability to a file cannot discover that the file exists.
- Pulling power mid-write leaves the filesystem mountable — corruption is bounded and detected, not silent.
- Audit records persist and survive rotation.

**Evidence:** write/reboot/verify transcript; namespace-isolation denial log; power-loss recovery test output.

---

### Phase 5 — Agent Runtime Substrate

**Depends on:** Phase 4. **Blocks:** Phases 6–7.

This is the layer the whole project exists for. Everything before it was making the layer safe to build.

**Deliverables**

1. Agent process model — an ordinary user-space process holding a restricted capability set, with no special kernel privileges.
2. **Structured system introspection API.** The concrete answer to "agent-native rather than agent-on-top": processes, devices, storage objects, and services are enumerable and invocable as typed, machine-legible objects. An agent queries the system through a real interface; it never scrapes a UI, parses human-formatted text, or shells out to commands designed for people.
3. Tool/intent surface — capability invocations exposed as typed, discoverable operations with declared preconditions and effects.
4. Policy engine — declarative rules over which capabilities an agent may hold and exercise, enforced at grant time.
5. Agent-facing audit query interface, capability-scoped.

**Exit criteria**

- An agent process enumerates the system and invokes operations entirely through typed interfaces, with no text scraping anywhere in the path.
- A policy denial is enforced by the kernel's capability check, not by the agent's cooperation.
- Every agent action in a session is reconstructible from the audit log alone.
- A misbehaving agent is contained to its own process and its granted capabilities — demonstrated adversarially.

**Evidence:** introspection API transcript; policy-denial log; a full session reconstructed from audit records alone; adversarial containment test.

---

### Phase 6 — Driver Synthesis Loop

**Depends on:** Phase 5, and unconditionally on ADR-006 IOMMU support being complete and verified.

Deliberately placed here, not earlier. Every prerequisite that makes agent-generated driver code *safe to execute* — user-space isolation (P3), IOMMU DMA containment (P3), capability scoping (P2), audit trail (P2), restart-on-crash (P3) — now exists. Running this phase before them means running agent-written code with none of the containment the design claims.

**Deliverables**

1. Snapshot-restore test harness: each attempt runs against a disposable VM snapshot; a panic or hang costs a snapshot restore, never persistent state.
2. Failure-capture pipeline: kernel log, driver process state, IOMMU violations, and timeout detection fed back as structured input to the next attempt.
3. Bounded retry: five attempts, then halt and hand a human the complete attempt history.
4. First target — `virtio-net` on Tier 1, with the in-tree driver absent. Spec plus PCI configuration space as the only inputs.
5. Only after Tier 1 succeeds repeatedly: a physical-hardware harness with power-cycle and rollback, and a documented device-class allowlist. Devices whose failure modes are irreversible — firmware flashing, embedded controller commands — are excluded, permanently.

**Exit criteria**

- Agent-synthesized `virtio-net` driver loads, brings the link up, and passes traffic in QEMU.
- An induced failure is captured, fed back, and corrected within the retry budget.
- Exhausting the budget produces a clean halt with a complete log trail — never a corrupted system.
- A synthesized driver attempting out-of-domain DMA is blocked by the IOMMU, and the block appears in the audit log.

**Evidence:** full attempt-history log per synthesis run; link-up and traffic-pass transcript; induced-failure recovery trace; IOMMU containment log.

---

### Phase 7 — Shell And Operator Surface

**Depends on:** Phase 5.

**Deliverables**

1. Text shell as a user-space process — inspect processes, memory, capabilities, devices, storage, and audit log.
2. Capability-aware command surface: the shell can only do what its capabilities permit; it is not privileged by being the shell.
3. Human-readable audit log viewer.
4. Natural-language intent path routed through the Phase 5 typed tool surface — resolving to the same capability invocations a human-typed command would, never a separate privileged path.

**Exit criteria**

- Full system state is inspectable from the shell.
- A shell command and the equivalent agent-issued intent produce identical audit records.
- Shell operations exceeding its capabilities are denied.

**Evidence:** shell session transcript; paired human/agent audit records demonstrating equivalence.

---

### Phase 8 — Hardware Consolidation And Release

**Depends on:** Phases 6 and 7.

**Deliverables**

1. Tier 2 machine fully supported: storage, input, display, network.
2. Release image build with reproducible-build verification.
3. Full automated suite: `test-boot`, `test-host`, `test-faults`, `test-integration`, `test-release`.
4. Documented supported-hardware list — narrow and honest.
5. Boot-time and I/O throughput measurements captured as tracked regression baselines.

**Exit criteria**

- Tier 2 hardware boots to shell with working storage, input, display, and network.
- The full suite passes from a clean checkout with no manual steps.
- Release image is reproducible from source.

**Evidence:** physical hardware boot log; complete suite output; reproducible-build hash comparison; performance baseline file.

---

**Phase 8 is where the original roadmap ended.** What follows (Phases 9–15) is new scope, added on request, to carry the project from "Tier 1/Tier 2 verified, code-complete" to "a real, daily-driveable OS" — the bar Linux, macOS, and Windows actually clear. None of it was implied by Phase 8's own exit criteria; §6's dependency structure is extended, not reinterpreted.

---

### Phase 9 — Multi-Core Execution (SMP)

**Depends on:** Phase 8. **Blocks:** Phases 10–15.

Every phase through 8 was explicitly single-core (§6, old dependency diagram; `docs/SUPPORTED_HARDWARE.md`'s own stated exclusion). No real machine anyone would call "a real OS" runs on one core today. This is the first new-scope phase and it is foundational to all the others — a network stack, a compositor, and a package manager all assume they can run concurrently with everything else, which single-core cooperative-ish scheduling does not actually give you under load.

**Deliverables**

1. AP (application processor) bring-up via the real INIT-SIPI-SIPI sequence per the MP/ACPI MADT tables already parsed since Phase 3's ACPI work — every core the firmware reports, not an assumed count.
2. Per-CPU kernel data (GDT/TSS/IST/local scheduler state) — no shared mutable kernel state without an explicit lock or per-CPU duplication.
3. SMP-safe scheduler: per-core run queues, real load balancing (even a simple periodic rebalance is acceptable — starvation is not), IPI-based reschedule.
4. TLB shootdown via IPI on every cross-core mapping change — a correctness requirement, not an optimization, given Phase 0's per-page permission model.
5. Every existing shared kernel structure from Phases 0–8 audited and made SMP-safe: the capability table (Phase 2), the audit log (ADR-005), IOMMU domain assignment (ADR-006/Phase 3), the PMM/heap (Phase 0). This is real, unglamorous, load-bearing work — a race here undermines every safety guarantee every earlier phase built.
6. `kernel_common`-style host-side tests for anything in the above that is pure logic (lock-free structures, per-CPU index math) — same discipline as every phase before it.

**Exit criteria**

- All firmware-reported cores are brought up and independently execute real work — demonstrated by a per-core heartbeat log with distinct APIC IDs, not just a core count printed once.
- A deliberate cross-core race (two cores contending the same capability-table entry, injected the same way Phase 6's fault-injection harness injects driver faults) is caught, not silently corrupting state.
- TLB shootdown is demonstrated: a mapping torn down on one core is provably unusable on another core within a bounded time, not "eventually."
- The full Phase-8 regression suite still passes, now running under SMP, with no new flakiness introduced (a real risk — timing-sensitive tests written under single-core assumptions are exactly what breaks first).

**Evidence:** per-core heartbeat log with APIC IDs; race-injection soak-test transcript (pass and a captured pre-fix failure, same "prove the bug, then prove the fix" discipline the IOMMU per-bus bug used); TLB shootdown timing log; full regression suite re-run under SMP.

---

### Phase 9.5 — Self-Healing And Self-Improvement

**Depends on:** Phase 9. **Gated by ADR-009 and ADR-010 sign-off (§2) — do not start without both.** **Blocks:** Phase 10 (see below), and is a prerequisite for any future claim that this system improves itself.

**Why this phase exists, and why it is numbered 9.5 rather than appended:** it was inserted after Phase 9 closed, when a concrete gap was found while planning Phase 10 — **Phase 10's own deliverable 4 and fourth exit criterion require crash-and-restart of a real user-space process, and that machinery does not exist.** What exists is one half of it at each end and no wire between them: `idt.rs` catches a real ring-3 fault and kills exactly that process (real, evidenced since Phase 1), and `device_manager.rs` holds a real bounded restart-on-crash state machine (real, evidenced since Phase 3) — but nothing observes a real death and drives that state machine. `report_crash()` has only ever been called from a *deliberately simulated* crash in `main.rs`, which `device_manager.rs`'s own module doc states plainly rather than hides. The fractional number is deliberate: it records honestly that this phase was discovered mid-roadmap rather than planned from the start, and avoids renumbering Phases 10–15 in a way that would make already-written `docs/PROGRESS.md` history read falsely.

**Deliverables**

1. **A real in-OS supervisor.** Observes genuine process death — the actual `PROCESS_KILLED` path in `idt.rs`, not a simulated call — and drives `device_manager`'s existing restart policy against it. Restarts a real user-space driver process for real. Quarantines a component that exhausts its restart budget instead of restarting forever.
2. **Structured, queryable health history** as real typed kernel state: per-component crash counts, causes (fault vector, IOMMU violation, timeout), and outcomes, built on ADR-005's audit log rather than beside it, and exposed through Phase 5's typed introspection surface — never text-scraped.
3. **Tier A self-tuning (ADR-009).** Real behavior derived from that recorded history rather than from constants: restart backoff computed from a component's own real crash pattern, and least-privilege authority envelopes narrowed from real observed IOMMU fault addresses — the promotion of research §3 (`docs/RESEARCH_TRACK.md`) out of the research track into production, which is a decision this phase makes explicitly rather than by drift.
4. **Tier B adoption pipeline (ADR-009).** Pre-declared machine-checkable criterion; contained trial; promote-or-reject; full audit of both outcomes; durable rollback to the previous known-good version. Generalizes `scripts/test-synthesis.ps1`'s host-side loop into a real in-OS capability.
5. **Immutable-core enforcement (ADR-010).** Mechanical, not documentary. A proposal touching the enumerated core is refused and the refusal is audited.
6. **Tier C prior-art gate (ADR-009).** A proposal claimed as a research breakthrough must pass a recorded prior-art check before that claim may be attached to it, held to `docs/PRIOR_ART.md`'s existing standard.
7. **`kernel_common`-style host tests** for everything here that is pure logic — criterion evaluation, backoff derivation, exclusion-list matching, health-history accounting — same discipline as every phase before it.

**Exit criteria**

- A real user-space driver process is killed by a real fault; the supervisor observes that **real** death (evidence must distinguish this from the existing simulated path) and restarts it; unrelated processes are demonstrably unaffected.
- A component that keeps failing exhausts its budget and is quarantined, with the complete history retained — never an unbounded restart loop.
- A deliberately introduced bug in a non-core component is corrected by an adopted change, with the pre-declared criterion shown **failing before and passing after** — the falsifiability requirement of ADR-009 clause 2, demonstrated rather than asserted.
- A proposed change targeting the ADR-010 immutable core is refused, and the refusal appears in the audit log.
- A trial that fails and rolls back leaves the system **byte-identical** to its pre-trial state, verified by hash, using the same durability bar Phase 8's power-loss testing already meets.
- Tier A self-tuning demonstrably changes real behavior based on real recorded history — shown by two runs with different histories producing different, correct decisions, not by reading the code.

**Evidence:** real-death-to-restart transcript with the fault and the restart causally linked; quarantine-after-budget history; before/after criterion evaluation for the adopted bug fix; audited immutable-core refusal; pre/post-rollback hash comparison; two-history self-tuning comparison.

**Explicitly out of scope, so it cannot be implied later:** the OS does not author changes and does not perform research. ADR-009 Option C is the whole basis of this phase — every proposal originates outside the kernel, and the system's contribution is judgment, containment, and rollback. Any description of this capability that omits that is an overclaim.

---

### Phase 10 — Network Stack And Real Connectivity

**Depends on:** Phase 9, **and Phase 9.5** (deliverable 4 and the fourth exit criterion below both require the real crash-and-restart supervisor built there). **Blocks:** Phases 12–15 (Phase 11 does not depend on this one and may run in parallel).

Phase 6 already proved a NIC driver (`virtio-net`, then real `e1000`) can move a frame. Nothing above the link layer exists yet — no IP, no TCP, no sockets, no name resolution. A real OS is a networked OS.

**Deliverables**

1. A real, capability-scoped socket abstraction — a process holds a socket capability the way it holds any other object (ADR-003), never an ambient "the network is just there" model.
2. IPv4, ARP, ICMP, UDP, and TCP (a real, spec-compliant TCP — congestion control, retransmission, the actual state machine — not a toy subset that only demos well) as a user-space network-stack service, per ADR-001.
3. DNS resolution as a capability-scoped client of the stack, not a kernel primitive.
4. The stack itself treated as a driver-adjacent process: crash-and-restart per Phase 3's device-manager discipline, no special-cased trust.
5. Loopback and multi-NIC routing, since Tier 3 (Phase 11) machines will not all have one NIC.

**Exit criteria**

- A real HTTP GET against an external, non-QEMU-emulated server succeeds and the response is byte-verified.
- Two independent Agentic OS instances (two QEMU guests, then two Tier 2/3 machines) exchange TCP traffic directly.
- Revoking a process's socket capability mid-connection terminates its access immediately — demonstrated adversarially, same bar Phase 2's capability revocation was held to.
- The network-stack process is killed mid-transfer; it restarts; no other process's sockets are affected.

**Evidence:** real external HTTP transcript with byte-verified response; two-machine TCP exchange log; socket-revocation-mid-connection transcript; network-stack crash-and-restart log.

---

### Phase 11 — Hardware Breadth (Tier 3) And Power Management

**Depends on:** Phase 9. Gated by ADR-007 sign-off (§2) — do not start without it.

**Deliverables**

1. A published, bounded Tier 3 hardware matrix (§4 is extended, not replaced) — a handful of real, named machines spanning at least two chipset generations and both a laptop and a desktop class, chosen and grown the same way Tier 2 was: IOMMU-gated, evidenced, never assumed.
2. USB host controller support (xHCI) plus HID class drivers (keyboard, mouse, mass storage) built through the Phase 6 synthesis loop against the real xHCI spec — the same "spec plus config space, contained by IOMMU" discipline used for every driver since Phase 6, now exercised repeatedly instead of once.
3. ACPI power management: CPU C-states/P-states, and a real S3 suspend/resume path — state genuinely torn down and restored, not merely "the screen goes black."
4. Hot-plug device support (USB primarily) — the device manager (Phase 3) already restarts crashed drivers; this extends it to devices that appear and disappear at runtime, not just at boot.
5. `docs/SUPPORTED_HARDWARE.md` grows a real Tier 3 section with the same evidence bar Tier 1/Tier 2 sections have carried since Phase 8 — narrow and honest, explicitly not implying anything about hardware not on the list.

**Exit criteria**

- The OS boots to shell on every machine in the published Tier 3 matrix, with storage, input, and display working on each — individually evidenced, not asserted once and generalized.
- A USB keyboard and mouse work through the real xHCI/HID path, with the same driver-crash-and-restart guarantee Phase 3 established for built-in devices.
- A suspend/resume cycle preserves running process state and network connections — demonstrated, not assumed from the ACPI call succeeding.
- A device hot-plugged mid-session is detected, bound to a driver, and usable without a reboot; unplugging it does not fault the system.

**Evidence:** per-machine boot log for every Tier 3 entry; USB HID input transcript; suspend/resume state-preservation transcript; hot-plug attach/detach log.

---

### Phase 12 — Display Server And Agent-Native Visual Surface

**Depends on:** Phase 9 (SMP — a compositor without it is a bad idea) and Phase 3 (GPU/framebuffer driver path). Gated by ADR-008 sign-off (§2) — do not start without it. **Blocks:** Phase 13.

**Deliverables**

1. A compositor as an ordinary capability-holding user-space service (ADR-008) — window and surface objects are typed, capability-scoped, and enumerable through the same introspection model Phase 5 built for everything else.
2. GPU 2D-acceleration and framebuffer drivers grown through the Phase 6 synthesis loop, IOMMU-contained like every other driver — 3D acceleration is explicitly out of scope for this phase (a real, separate, much larger undertaking; do not silently fold it in).
3. Input routing (keyboard/mouse/touch, from Phase 3's PS/2 and Phase 11's USB HID) to the focused, capability-holding window — a window cannot receive input for a surface it does not hold a capability to.
4. A minimal native UI toolkit against the capability ABI (ADR-004) — text rendering, basic widgets — enough to build the reference apps Phase 13 needs, not a general framework.
5. The typed UI object model extends Phase 5's agent introspection API: an agent enumerates windows, reads their content as structured data, and issues input events through the same typed interface — explicitly never pixel-scraping or OCR-over-screenshot as a supported path.

**Exit criteria**

- Multiple windowed processes run concurrently with provably isolated surfaces — one process cannot read another's window buffer, demonstrated adversarially, the GUI analogue of Phase 1's process-memory-isolation exit criterion.
- An agent process manipulates a window (reads its content, sends input) entirely through the typed API, with an audit trail identical in kind to any other capability invocation (ADR-005) — no screen-scraping code path exists to fall back to.
- A crashed compositor is restarted by the device manager without taking down running application processes' own state (their windows may need to be recreated — document exactly what survives and what doesn't, honestly).
- Human keyboard/mouse input reaches the correct focused window with no cross-window leakage.

**Evidence:** window-isolation adversarial-test transcript; agent-driven UI session log showing typed API calls only; compositor crash-and-recover transcript; input-routing correctness log.

---

### Phase 13 — Native Application Platform And Package Ecosystem

**Depends on:** Phase 12 (and Phase 10 for any app requiring network access).

A GUI and a network stack with nothing to run on them is not yet a usable OS. This phase is what makes it one, on this OS's own terms — explicitly not POSIX/Linux/Windows binary compatibility (§1.3, unchanged).

**Deliverables**

1. An application manifest format declaring required capabilities up front — install-time, explicit, least-privilege by construction (closer to a mobile-OS permission model than classic desktop ambient trust), consistent with ADR-003 from the very first syscall this OS ever had.
2. A package store/installer service, itself an ordinary capability-holding process — no special ambient install-time privilege.
3. A native SDK/toolchain targeting the real capability ABI (ADR-004's library, extended, not replaced) — documented well enough that an app can be built without reading kernel source.
4. A small set of reference applications built entirely on the public ABI, proving the platform is usable beyond the shell: a file manager, a terminal emulator (fronting Phase 7's shell), a text editor, and a simple network client (using Phase 10's stack). These are dogfooding, not filler — every one of them should surface real ABI gaps the way `virtio_blk_driver` surfaced real toolchain bugs.
5. Update/versioning for installed apps, reusing Phase 8's reproducible-build discipline for app packages, not inventing a separate one.

**Exit criteria**

- Installing an app grants exactly the capabilities its manifest declared, nothing ambient — an app that tries to use an undeclared capability is denied, demonstrated adversarially.
- The SDK builds and runs a genuinely new application (not one of the four reference apps) without any kernel or platform-service change.
- All four reference apps run concurrently, each capability-isolated from the others, exercising GUI, storage, and network simultaneously.
- An app update is rejected if its build is not reproducible against its declared source, same evidence bar as Phase 8's release image.

**Evidence:** capability-manifest enforcement/denial transcript; third-party-style SDK app build-and-run log; concurrent multi-app isolation transcript; app-update reproducibility check.

---

### Phase 14 — Multi-User, Update Integrity, And Hardware Root Of Trust

**Depends on:** Phase 13.

**Deliverables**

1. A multi-user session model — still capability-based throughout, never uid/gid ambient authority (ADR-003 holds all the way up the stack, not just at the syscall layer).
2. Secure Boot chain integration with the existing UEFI boot path (`boot_rs`), plus measured boot into a TPM 2.0 PCR log where hardware provides one — narrow, honest support (Tier 2/3 machines with a TPM only; state plainly which don't).
3. A signed release/update mechanism with rollback, built on Phase 8's reproducible-build hashes as the integrity baseline — an update is a verified artifact, not a trusted network fetch.
4. Full-disk/object-store encryption (extending Phase 4's object store), keyed off the same hardware root of trust where available.
5. Opt-in, local-first crash/panic telemetry that feeds back into the Phase 6 synthesis loop's own failure-capture pipeline — this OS already has a real structured-failure-capture discipline; extending it to end-user crash reports is reusing that discipline, not building a new one.

**Exit criteria**

- Two local users' data and running processes are provably capability-isolated from each other — demonstrated adversarially, same bar every isolation claim in this roadmap has been held to.
- An unsigned or tampered update image is rejected before it is ever applied.
- A tampered boot chain (modified bootloader or kernel image) is detected and refused to boot, with measured-boot evidence where TPM hardware is present.
- Full-disk encryption round-trips a reboot with data byte-identical, same evidence bar as Phase 4's original write/reboot/verify test, now with encryption in the path.

**Evidence:** multi-user isolation adversarial transcript; rejected-unsigned-update log; tampered-boot-chain refusal log with TPM PCR evidence where applicable; encrypted write/reboot/verify transcript.

---

### Phase 15 — Long-Term Stability And Self-Hosting

**Depends on:** Phase 14. **This is the exit criterion for the entire roadmap — the phase where "a real, daily-driveable OS" stops being a claim and becomes a demonstrated property.**

**Deliverables**

1. A self-hosting toolchain: Agentic OS builds its own next release image while running natively on Agentic OS itself — closing the bootstrap loop that every cross-compiled phase before this one depended on.
2. Extended soak testing: multi-day continuous uptime under a real mixed workload (network + storage + GUI + agent activity concurrently), not a clean-boot-and-shut-down test like every `scripts/test-*.ps1` before it.
3. A documented compatibility and regression policy — what "supported" means going forward, how a Tier 3 machine or a driver gets added or removed from the matrix, and what breaks that policy (mirrors the honesty discipline `docs/SUPPORTED_HARDWARE.md` has held since Phase 8, made explicit as a standing policy rather than a per-release document).
4. A real out-of-box installer for Tier 2/3 hardware — partitioning, filesystem creation, bootloader install — built on this OS's own tooling, not a separate one-off script.
5. A final, honest public compatibility matrix: exactly what this OS supports, next to an explicit statement of what it still does not (multi-architecture, POSIX binary compatibility, and in-kernel model inference remain permanently out of scope per §1.3, unchanged through this entire phase).

**Exit criteria**

- Agentic OS builds its own release image, running natively, and the output is verified reproducible against the cross-compiled release from Phase 8's original toolchain — the strongest form of "this OS is real" this roadmap can produce.
- An N-day (minimum 7, target 30) continuous soak run under the real mixed workload completes with zero unexplained crashes — every crash either explained and fixed, or explained and documented as a known, tracked limitation. No unexplained crash is waved away as "probably fine."
- A fresh install onto Tier 2/3 hardware via the real installer, from nothing, boots to a usable multi-app GUI session — the single demonstration that most directly answers "is this a real OS."

**Evidence:** self-hosted build reproducibility comparison; multi-day soak-test log with full crash accounting; end-to-end fresh-install-to-usable-desktop transcript on real hardware.

---

## 6. Dependency Structure

```
ADR sign-off
     │
     ▼
  Phase 0 ── Foundation Correctness ──────── (blocks everything)
     │
     ▼
  Phase 1 ── Execution Model
     │
     ▼
  Phase 2 ── Capabilities + IPC + Audit ──── (the agent-native decision point)
     │
     ▼
  Phase 3 ── User-Space Drivers + IOMMU
     │
     ▼
  Phase 4 ── Storage + Filesystem
     │
     ▼
  Phase 5 ── Agent Runtime Substrate
     │
     ├──────────────┐
     ▼              ▼
  Phase 6        Phase 7
  Driver         Shell +
  Synthesis      Operator Surface
     │              │
     └──────┬───────┘
            ▼
        Phase 8 ── Hardware Consolidation + Release
            │
            ▼
        Phase 9 ── SMP (Multi-Core) ──────────── (foundational to everything below)
            │
     ┌──────┴───────────────┐
     ▼                      ▼
  Phase 9.5              Phase 11
  Self-Healing +         Hardware
  Self-Improvement       Breadth (Tier 3)
  │ ADR-009 + ADR-010    │ ADR-007 gate
  │ gate                 │
     │                   │   (Phase 11 needs neither 9.5 nor 10)
     ▼                   │
  Phase 10               │
  Network Stack          │
  (needs 9.5's real      │
   crash-and-restart)    │
     │                   │
     └──┬────────────────┘
        ▼
    Phase 12 ── Display Server (ADR-008 gate, needs Phase 9 + Phase 3's GPU path)
        │
        ▼
    Phase 13 ── Native App Platform (needs Phase 10 for network-capable apps)
        │
        ▼
    Phase 14 ── Multi-User + Update Integrity + Root of Trust
        │
        ▼
    Phase 15 ── Long-Term Stability + Self-Hosting ── (the roadmap's exit criterion)
```

There is no parallel track that skips ahead. Phase 6 in particular cannot be pulled forward: it depends on containment guarantees established in Phases 2 and 3, and running it without them means running agent-written code with no isolation whatsoever. The same rule applies below Phase 8: Phase 11 cannot start without ADR-007 sign-off, and Phase 12 cannot start without ADR-008 sign-off — both are named exceptions to §1's non-goals, not routine phase transitions, and skipping the sign-off step to save time reintroduces exactly the risk §4 and ADR-001 already warned about.

---

## 7. Open Decisions Requiring Sign-Off

| # | Decision | Recommendation | Cost of deciding late |
|---|---|---|---|
| 1 | ADR-002 — implementation language | Rust for new kernel work; C fully defensible if it is faster in your hands | Grows with every line written. Effectively permanent after Phase 1 |
| 2 | ADR-001 — microkernel-leaning split | Adopt | Retrofitting user-space drivers after Phase 3 is a rewrite of every driver |
| 3 | ADR-003 — capability ABI from the first syscall | Adopt | Cannot be retrofitted. Rewrites every syscall and every caller |
| 4 | ADR-006 — IOMMU before agent-generated drivers | Adopt | Blocks Phase 6 entirely; determines Tier 2 hardware eligibility |
| 5 | Tier 2 physical machine | Choose at Phase 3, gated on IOMMU + early serial | Low if deferred to Phase 3; high if hardware is bought before the criteria are set |
| 6 | Filesystem: implement documented format vs. design new | Implement a documented format | Medium — a custom format costs debugging tools you would have to write |
| 7 | ADR-007 — reverse the hardware-breadth non-goal, narrowly (Tier 3) | Adopt, bounded to a published matrix | Blocks Phase 11 entirely; deciding late just delays "runs on more than one machine," no compounding cost |
| 8 | ADR-008 — reverse the GUI non-goal, as a capability-native compositor only | Adopt Option C, reject Option B (ported conventional compositor architecture) | Blocks Phase 12 entirely; choosing Option B late would mean a second, incompatible security model bolted onto the OS — effectively as permanent a mistake as getting ADR-002 wrong |
| 9 | ADR-009 — self-modification is evidence-gated adoption; the OS judges, never authors | Adopt Option C, reject Option A (in-kernel authorship) outright — it violates §1.3 and removes the only independent check | Blocks Phase 9.5, which in turn blocks Phase 10. Choosing Option A late is unrecoverable: once the judge and the author are the same component, no later evidence from that system can be trusted |
| 10 | ADR-010 — a permanently immutable core, excluded from self-modification | Adopt Option C. The list may only ever be extended, never narrowed by a running system | Blocks Phase 9.5 alongside ADR-009. Deciding late, or too narrowly, means a single bad change can disable the machinery that would have caught it — and the system keeps reporting success afterward |

---

## 8. Review Discipline

The acceptance and rejection rules already in `plans_to_implement/antigravity_prompts/13_ORCHESTRATOR_ACCEPTANCE_CHECKLIST.md` remain in force and apply to every phase gate here. Three additions specific to this roadmap:

- **Reject any change that reintroduces ambient authority.** A syscall that succeeds because of who the caller is rather than what capability it holds violates ADR-003, regardless of how convenient it is.
- **Reject any driver code in ring 0** after Phase 3, other than the interrupt dispatch stub itself.
- **Reject any agent-generated driver execution** — in QEMU or on hardware — before ADR-006 IOMMU support is complete and its containment has been demonstrated.

A phase gate passes on demonstrated evidence, not on a report that the work is done.
