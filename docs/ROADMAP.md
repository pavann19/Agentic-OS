# Agentic OS — Build Roadmap

**Status:** Proposed — requires sign-off on Section 2 before Phase 0 begins
**Date:** 2026-08-27
**Supersedes:** the previous `docs/ROADMAP.md` (preserved in git history at commit `6818872`)
**Scope:** ground-up construction plan derived from the code actually in this repository

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
```

There is no parallel track that skips ahead. Phase 6 in particular cannot be pulled forward: it depends on containment guarantees established in Phases 2 and 3, and running it without them means running agent-written code with no isolation whatsoever.

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

---

## 8. Review Discipline

The acceptance and rejection rules already in `plans_to_implement/antigravity_prompts/13_ORCHESTRATOR_ACCEPTANCE_CHECKLIST.md` remain in force and apply to every phase gate here. Three additions specific to this roadmap:

- **Reject any change that reintroduces ambient authority.** A syscall that succeeds because of who the caller is rather than what capability it holds violates ADR-003, regardless of how convenient it is.
- **Reject any driver code in ring 0** after Phase 3, other than the interrupt dispatch stub itself.
- **Reject any agent-generated driver execution** — in QEMU or on hardware — before ADR-006 IOMMU support is complete and its containment has been demonstrated.

A phase gate passes on demonstrated evidence, not on a report that the work is done.
