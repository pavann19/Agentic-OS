# Agentic OS Progress Tracker

Source: `docs/ROADMAP.md` (the ADR-driven build plan, 2026-08-27), tracked
here phase-by-phase against what's actually built and evidenced — same
discipline as the repo audit from 2026-08-29: nothing here is marked done
without a real, reproducible artifact behind it (a boot log, an `objdump`
result, a passing test run). Detailed narrative evidence lives in
`PHASE0_PROGRESS.md` and `NATIVE_BUILD.md`; this file is the checklist view
across the whole roadmap, updated in the same commit as whatever changed it.

Status marks: `[x]` done and evidenced · `[~]` in progress / partially done
· `[ ]` not started.

---

## Phase 0 — Foundation Correctness (9/9 items complete — DONE)

Depends on: ADR sign-off (done — ADR-001/002/003/006 accepted, ADR-002
specifically extended mid-build to cover the bootloader too, see
`NATIVE_BUILD.md`). Completed 2026-08-30, overnight session, ~1h02m real
elapsed time from a standing start (kernel with only boot-info validation)
to all 9 items done and evidenced.

- [x] **Toolchain decision (ADR-002) executed** — Rust, both bootloader and
      kernel. Native, Docker-free: `rustup` (nightly, GNU ABI) + `QEMU` via
      `winget`, no MSVC/gnu-efi/mtools. See `NATIVE_BUILD.md`.
- [x] **Bootloader ported to Rust** — full port of `boot/main.c`'s loading
      logic: hand-written UEFI bindings (BootServices, LoadedImage,
      SimpleFileSystem, File, GraphicsOutput protocols), ELF64 PT_LOAD
      segment mapping to exact physical addresses, PSF1 font loading, GOP
      framebuffer discovery, BootInfo construction, GetMemoryMap/
      ExitBootServices retry loop.
- [x] **Kernel ported to Rust (boot-critical slice)** — real
      `BOOT_START → EXIT_BOOT_SERVICES_OK → KERNEL_ENTER` chain, validates
      the real `BootInfo` handed to it.
- [x] **All 32 CPU exception vectors + TSS/IST double-fault path**
      (`gdt.rs`, `idt.rs`) — real TSS with a dedicated double-fault IST
      stack, all 32 vectors via Rust's native `x86-interrupt` ABI. Verified
      by the fault-injection suite, including a real double-fault via
      genuine stack overflow, correctly caught on the IST stack.
- [x] **VMM redesign** (`vmm.rs`, `pmm.rs`) — higher-half kernel
      (`0xFFFFFFFF80000000`), real per-segment permissions (NX on
      data/rodata, executable only on `.text`), physical direct-map window
      replacing the old blanket identity map, real `unmap_page`+`invlpg`,
      null-guard (page 0 never mapped). `VMM_INIT_DONE` verified live.
- [x] **Kernel heap** (`heap.rs`) — hand-written linked-list allocator,
      `#[global_allocator]`, real `alloc::vec::Vec` smoke test with a
      mathematically-verified result (Σi², i=0..256 = 5559680).
- [x] **Local APIC timer** (`apic.rs`, `pic.rs`) — periodic tick at vector
      0x20, legacy PIC remapped and masked to avoid dual-firing. Verified
      live (`TIMER_TICKS_OBSERVED count=3`).
- [x] **Deferred interrupt work** (`events.rs`) — lock-free SPSC ring
      buffer; `h_timer`'s entire job is bump-counter/push-event/EOI.
      Producer (interrupt context) → consumer (main loop) path verified
      end to end.
- [x] **Host test harness** (`kernel_common/`, `host_tests/`) — 23 real
      `cargo test` tests against the actual pure logic `kernel_rs` runs
      (shared via a path dependency, not a parallel reimplementation).
      Several are direct regression tests for bugs found this session.
- [x] **Fault-injection suite** (`make test-faults`) — 4/4 cases pass:
      null-deref, rodata-write, NX-exec (all page faults, correctly
      decoded), and a genuine double-fault via real stack overflow, caught
      on the IST stack. Found and fixed two real bugs building it (LLVM
      tail-call-optimizing away the intended stack overflow, and a real
      IST array-index-vs-IDT-gate-value off-by-one that left the
      double-fault handler running on the same exhausted stack it was
      supposed to be rescued from).

**Phase 0 exit criteria (`docs/ROADMAP.md` §5) — all met, verified via
`make clean && make test-boot && make test-host` plus the fault suite, all
passing from a fully clean tree:**
- Null dereference faults (vector 14, cr2=0). Write to kernel `.text`/
  `.rodata` faults. Execute on NX `.data` faults. Double fault produces a
  full diagnosed panic via the IST stack, not a hang.
- Kernel runs from the higher half; no blanket identity map remains in the
  kernel's own (post-switch) tables.
- Timer interrupts fire at a measured, observed rate.
- The one interrupt handler that exists (timer) does no unbounded work —
  a real deferred-event queue exists for every future interrupt source.
- `make test-host` executes 23 real assertions against the actual PMM/VMM
  logic kernel_rs runs, not a stub.

**Nine real bugs found and fixed getting here** (see `PHASE0_PROGRESS.md`
for full narrative): PMM span calc counting an MMIO/reserved descriptor at
~1TB; a physical-address-0 sentinel bug; a linker-symbol alignment bug that
put NX on live executing code; a missing stack mapping across the CR3
switch; an LLD orphan-section (`.got`) landing unaligned; an LLVM
tail-call-optimization hiding an intended stack overflow; and the IST
array-index/IDT-gate-value off-by-one. Every one was root-caused with real
evidence (`qemu -d int`, PTE readbacks, diagnostic serial output) before
being fixed, not guessed at.

---

## Phase 1 — Execution Model (5/5 items complete — DONE)

Depends on Phase 0 (done). Started 2026-08-30.

- [x] **Kernel threads + preemptive scheduler** (`thread.rs`) — real
      context switch (swap callee-saved regs + RSP, then `ret`), timer-
      driven preemption via `idt.rs::h_timer` calling `schedule()`.
      Verified live: two demo threads produce perfectly interleaved output
      (A0,B0,A1,B1,...), not sequential execution. Two real bugs found and
      fixed: a naked-vs-normal-function trampoline bug, and threads
      silently inheriting interrupts-disabled forever (fixed with `sti`
      before `switch_to`'s `ret`, safe via x86's STI-shadow guarantee).
- [x] **Per-process address spaces** (`vmm::new_address_space`) — fresh
      PML4 per process, upper half (canonical-high, indices 256-511)
      shared with the kernel, lower half independent per process.
      Verified live: two address spaces, identical virtual address
      (`0x400000`), different physical content per process
      (`0xAAAA...`/`0xBBBB...`) — real isolation, not just plumbing.
- [x] **Ring 3 execution** (`ring3.rs`) — `enter_user_mode()` via `iretq`.
      Verified live via a deliberate proof: user code executing `hlt`
      (CPL0-only) correctly raises `#GP` with `CS=0x33` (the exact user
      code selector) — unambiguous proof CPL was really 3. Two real bugs
      found and fixed: switching CR3 from kernel_main's own
      never-migrated boot-time stack double-faulted (fixed by requiring
      this to run in a spawned/heap-stacked thread), and intermediate
      page-table entries never carried the USER bit (only the leaf did),
      which x86_64 requires at every level for CPL3 access to succeed at
      all.
- [x] **`SYSCALL`/`SYSRET` entry path** (`syscall.rs`) — single-core
      simplification stated plainly (a plain static kernel-stack pointer,
      not the `swapgs`/per-CPU-GS mechanism multi-core kernels need; a
      real, load-bearing constraint to revisit before SMP, not a corner
      cut). Verified live with a complete, correct round-trip: user code
      executing `syscall` with `rdi=0x1234, rax=1` produces
      `SYSCALL_LOG value=0x1234` from the kernel, then `SYSRET` returns to
      `rip=0x60000c` — the EXACT byte after the `syscall` instruction —
      still at `cs=0x33` (ring 3), where the program's trailing `hlt`
      faults exactly as the ring-3-only demo's did. No bugs found
      building this one — the intermediate-page-table and stack-placement
      lessons from the ring-3 item applied directly. User-pointer
      validation stays explicitly open: no syscall here takes a pointer
      argument yet, so there's nothing to validate.
- [x] **Process lifecycle: create, exit, reap** (`thread.rs`) — `spawn`/
      `spawn_in` (create), `exit_current` (exit) already existed; reap was
      a **real bug found and fixed**, not a missing feature added cleanly:
      dropping an exited thread's `Box<Thread>` inline, on the same stack
      frame that's about to call `switch_to()` and abandon that frame
      permanently, meant the destructor never ran — every thread that
      ever exited leaked its `Box<Thread>` and 64KB stack. Fixed with a
      deferred zombie slot, reaped at the top of the NEXT `schedule()`
      call (which always runs on a different, still-valid stack — the
      same pattern Linux's `finish_task_switch` uses). Verified live: all
      four demo threads (two long-running, two address-space-isolation
      probes) produce `THREAD_REAPED id=N` for every one of them, in the
      order they actually exited.

**Phase 1 exit criteria (`docs/ROADMAP.md` §5)** — the concurrency and
isolation criteria are met and demonstrated (two threads preempted by the
timer; one process provably cannot read another's memory). Two criteria
remain explicitly unmet by design, not by oversight, and are correctly
Phase 2+ scope: a user-space fault terminating only that process while the
system continues (every unhandled exception still halts the whole kernel —
Phase 0's exception handlers were built as "diagnose and halt," and
"diagnose and recover into killing one process" needs the capability/IPC
substrate Phase 2 builds, not just more Phase 1 code); and a syscall
rejecting a malicious pointer argument (no syscall in this proof-of-concept
takes a pointer argument at all yet).

## Phase 2 — Capability And IPC Substrate (6/6 items complete — DONE)

Depends on Phase 1 (done). Completed 2026-08-30. This is the phase where
the OS becomes agent-native rather than a conventional kernel — the
decision point `docs/ROADMAP.md` itself frames it as.

- [x] **Per-process capability table, unforgeable references**
      (`capability.rs`) — `CapabilityTable` holds `Capability { object_id,
      rights, generation }` behind an opaque `CapId` index; nothing a
      process holds is a raw pointer or a guessable handle.
- [x] **Grant / attenuated derive / revoke** — `grant()` mints from a new
      object; `derive()`/`derive_self()` enforce attenuation as a REJECTED
      request, not a silent clamp, when the requested rights aren't
      already a subset; `revoke()` bumps the object's generation counter.
      Revocation is immediate for every derivative, by construction —
      derived capabilities share the source object's generation, so one
      write invalidates the whole tree with no walk needed.
- [x] **Synchronous, capability-gated IPC** (`ipc.rs`) — real rendezvous
      (spin-yield, documented single-core simplification), gated through
      the same `CapabilityTable::resolve` every other operation uses.
- [x] **Kernel audit log, one code path with invocation** (`audit.rs`,
      ADR-005) — `capability.rs`'s grant/derive/revoke and `resolve`'s
      denial path all call `audit::record` as part of their own bodies;
      there is no capability operation whose code skips it.
- [x] **Interrupt forwarding to a registered handler** (`interrupt_forward.rs`)
      — real hardware interrupt (the APIC timer, this kernel's one live
      IRQ source), `notify()` called from the actual ISR, `wait_for_interrupt`/
      `acknowledge` as two distinct, separately-audited steps.
- [x] **Syscall surface expressing a capability operation** — syscall
      number 2 (`syscall.rs`) performs a real, capability-gated IPC send
      from ring 3, enforcement not bypassed for the syscall path.

**Every exit criterion (`docs/ROADMAP.md` §5) demonstrated live, in one
boot log, not just individually:** a denied access (SEND-only capability
rejected for RECEIVE), an attenuation violation rejected outright (not
clamped), immediate revocation (same `cap_id`, post-revoke, denied), and a
complete audit trail with no gaps (11 records for the kernel-thread demo
alone — Grant, 3×Derive, 2×Denied, IpcSend, IpcReceive, Revoke,
InterruptDelivered, InterruptAcknowledged — sequential, none missing).

**Six real bugs found and fixed getting here**, several substantially
deeper than Phase 0/1's: (1) the first attenuation-violation test
exercised the wrong failure path entirely (missing `GRANT`, not an
attenuation violation) — required understanding `derive()`'s own
precondition, not a typo; (2) a genuine memory-visibility bug in the IPC
rendezvous — `spin_yield()`'s `options(nomem, nostack)` let LLVM cache a
plain field read across the wait loop, spinning on a stale register
forever, fixed with real atomics and Acquire/Release ordering; (3) a
**deadlock**: `IA32_FMASK` masks interrupts on syscall entry, and a
blocking syscall (IPC send) needs the timer to fire for the scheduler to
ever unblock it — fixed with `sti` inside the entry stub, using the same
STI-shadow reasoning as Phase 1's thread-switch fix; (4) fixing that
deadlock exposed a second, deeper one: syscall entry swapped onto a
*shared scratch stack* separate from the calling thread's own tracked
stack, so a mid-syscall preemption saved the wrong RSP into the thread's
context — fixed by reusing the thread's own kernel stack instead
(`thread::current_kernel_stack_top`); (5) a thread that manually calls
`vmm::switch_address_space` (entering a freshly-created process's address
space at runtime) was never reflected in `thread.rs`'s own tracking, so
the NEXT preemption silently switched CR3 back to the stale value —
fixed with `thread::set_current_address_space`; (6) a phantom, undefined
helper function referenced during first-draft aliasing avoidance, caught
immediately by the compiler and replaced with a real `derive_self()`
method. Every one of (2)-(5) was found specifically because the
ring-3-plus-syscall demo was pushed to survive REAL preemption mid-
operation, not just a single uninterrupted happy path.

`make clean && make test-boot && make test-host && (fault suite)` all
pass with zero regression; the `demo_ring3` feature build (gated, same as
Phase 1's ring-3 proof) shows the complete capability→IPC→syscall→ring-3
round-trip, including surviving a real timer preemption mid-syscall.

## Phase 3 — User-Space Driver Framework (6/7 items complete, 1 partial — IN PROGRESS)

Depends on Phase 2 (done). Started 2026-08-30.

- [x] **Driver process model: MMIO/interrupt/port-IO capabilities**
      (`driver.rs`, extends `capability.rs`) — three new
      `KernelObjectKind` variants (`MmioRegion`, `InterruptLine`,
      `PortIoRange`), each mediated through the same `CapabilityTable`
      the rest of the kernel uses — no separate driver-privilege path.
      `map_mmio`/`wait_interrupt`/`ack_interrupt`/`grant_port_access` all
      resolve through a capability first; a real TSS I/O Permission
      Bitmap (1024 ports, COM1 included) backs port-IO grants — not a
      blanket `IOPL=3`.
- [x] **Real PCIe enumeration** (`pci.rs`) — Configuration Mechanism #1
      (CF8/CFC), full 256-bus × 32-device × 8-function scan. Verified
      live: found exactly the 6 real devices QEMU's Q35 machine actually
      exposes (host bridge, VGA, an unnamed 0x8086/0x10d3 function, ICH9
      LPC, ICH9 SATA/AHCI at 00:1f.2, ICH9 SMBus) — not a stub list.
- [x] **IOMMU bring-up** (ADR-006 hard gate) (`acpi.rs`, `iommu.rs`) —
      real ACPI RSDP/XSDT walk to find the DMAR table, real DRHD MMIO
      register discovery and mapping, real GCMD/GSTS hardware handshake
      (SRTP+poll RTPS, TE+poll TES), real per-device DMA domain page
      tables, `assign_device()` used against the actual SATA controller
      `pci::enumerate()` found (00:1f.2). Verified live: translation
      enabled, root table live, domain assigned with a real mapped
      range. **Scope note:** this proves the register-level mechanism
      end-to-end; it does not yet prove *behavioral* containment (an
      actual illegal DMA attempt being blocked and logged), since no
      DMA-capable driver exists yet to generate one — deferred to when a
      real AHCI driver exists to exercise it.
- [x] **Device manager** (`device_manager.rs`) — real PCI class/subclass/
      prog_if -> `DriverKind` classification, a real per-device lifecycle
      state machine (`Discovered -> Bound -> Running -> Crashed ->
      Restarting -> Failed`), and a bounded restart-on-crash policy
      (`MAX_RESTARTS=3`, then permanently `Failed` rather than retried
      forever). Verified live: discovers and binds all 5 classifiable
      devices from this session's own PCI scan, then a deliberately
      simulated crash+restart cycle against the real SATA/AHCI controller
      (00:1f.2) exercises the full state machine.  **Scope note:**
      `bind_all()` records a real binding decision but does not yet hand
      a capability set to an actual user-space driver process — that
      handoff is the next item below, still not started.
- [x] **`init` and a service manager as the first user-space processes**
      (`init.rs`, `service_manager.rs`) — `init` is a real, unconditional
      (not feature-gated, unlike the Phase 1/2 `demo_ring3` proof) ring-3
      process, part of normal boot now. It issues a real capability-gated
      syscall (SVC_START, syscall 3) that only succeeds once the service
      manager thread is alive and listening on its own capability
      endpoint — a real cross-process handoff, not a hardcoded boot-order
      assumption. The service manager reads the REAL `device_manager`
      state from this session's own PCI scan and decides what to start.
      Verified live: `INIT_ENTER` → `SERVICE_MANAGER: got SVC_START` →
      four real bound devices each getting a real "would start driver"
      decision. **Scope note:** the service manager itself still runs in
      ring 0 (a kernel thread), not ring 3 — real ring-3 driver processes
      need the ELF loader the next item below will build; duplicating
      that loader here just to move this one process to ring 3 would be
      wasted work.
- [~] **First user-space drivers** (serial, framebuffer, PS/2 keyboard),
      ported off the current in-kernel implementations — **serial done,
      framebuffer and PS/2 keyboard not started.** `kernel_rs/src/elf.rs`
      (new) is a real ELF64 loader — validates the header, walks PT_LOAD
      program headers, maps each with the same real permission
      discipline every other mapper in this kernel uses. `user_rs/
      serial_driver/` (new crate) is a genuine standalone ELF64 binary,
      built completely separately from the kernel, doing real unmediated
      ring-3 port I/O to COM1 after a one-time capability-gated IOPB
      grant. Verified live, unambiguously: the driver's own raw string
      appears DIRECTLY in the serial log, written via real `out`
      instructions — never touched `klog_info!` — proof this is genuine
      CPL3 code loaded from a real compiled ELF and entered at ITS entry
      point (`0x500000`, from the ELF header), not another
      kernel-hardcoded demo. **Real bug found and fixed:** the linker
      script packed `.text`(RX) and `.rodata`(R) into the same page since
      the binary is tiny; `elf.rs` maps PT_LOAD segments page-by-page, so
      the second segment's mapping silently clobbered the first's
      executable permission, producing an instruction-fetch `#PF` at the
      entry point on first boot — fixed with `ALIGN(4096)` between output
      sections (plus a `build.rs` so cargo actually re-links when
      `linker.ld` changes, since it has no dependency edge on the linker
      script otherwise). The *current* serial/klog code in `boot_rs/` and
      `kernel_rs/` itself still runs in the bootloader/kernel directly —
      correct, since that's boot-time diagnostics, not this driver.
- [ ] **Tier 2 physical machine selected and brought to serial output**
      (`docs/ROADMAP.md` §4 — needs IOMMU present, serial reachable,
      NVMe/AHCI storage, documented chipset) — not started.

**Three real bugs found and fixed this phase so far:** (1) `info:
&BootInfo`, validated under `boot_rs`'s bootstrap identity mapping, was
read again after `vmm::init()` switched CR3 to the kernel's production
tables — the old mapping no longer existed and the reference silently
dangled; fixed by re-deriving the reference through
`pmm::p2v_pub(boot_info as u64)` post-init. (2) E0793 "reference to field
of packed struct is unaligned" on DMAR remapping-structure fields — Rust
nightly hard-errors on this now; fixed with
`core::ptr::read_unaligned(core::ptr::addr_of!(...))`. (3) QEMU's default
`-machine q35` exposes no IOMMU/DMAR table at all; fixed by adding
`-device intel-iommu,intremap=on` and `kernel-irqchip=split` to
`scripts/test-boot.ps1`.

`make clean && make test-boot && make test-host` plus the fault
injection suite all pass with zero regression against this phase's work
so far.

## Phase 4 — Storage And Filesystem — Not started

Block device abstraction, on-disk filesystem, capability-scoped object
store, audit log persistence — none started.

## Phase 5 — Agent Runtime Substrate — Not started

Agent process model, structured introspection API, typed tool/intent
surface, policy engine, audit query interface — none started. This is the
layer the whole project exists for (per the original vision conversation);
everything before it is making it safe to build.

## Phase 6 — Driver Synthesis Loop — Not started

Depends on Phase 5 and unconditionally on ADR-006 IOMMU support. The
self-healing driver-agent loop discussed early in this project (spec →
generate → snapshot-test → retry ×5 → human escalation) has zero code
behind it — it was scoped as a milestone (`M-DRIVER-0`) in conversation
only, never started, and correctly sequenced last among the dependent
phases per the roadmap's reasoning (containment must exist before
agent-written driver code runs at all).

## Phase 7 — Shell And Operator Surface — Not started

Text shell, capability-aware command surface, audit log viewer,
natural-language intent path — none started.

## Phase 8 — Hardware Consolidation And Release — Not started

Tier 2 hardware full support, release image, full automated suite,
supported-hardware list, performance baselines — none started. Tier 2
hardware itself hasn't been selected yet (gated on IOMMU support existing,
per §7 of `docs/ROADMAP.md`).

---

## Where the project actually stands, in one paragraph

Phase 0 is complete and evidenced: a Rust UEFI bootloader that loads a real
kernel ELF and jumps to it, a Rust kernel with a working higher-half VMM
(real permissions, direct-map window, null-guard), a real heap, a real
timer with a deferred-event queue, all 32 CPU exceptions handled with a
correct IST-based double-fault path, a real host test suite, and a real
fault-injection suite — all passing from a fully clean tree
(`make clean && make test-boot && make test-host` plus the fault suite).
Phases 1 and 2 are also complete and evidenced: a preemptive kernel
scheduler with real isolated per-process address spaces, ring 3 execution,
`SYSCALL`/`SYSRET`, and process lifecycle (Phase 1); then a full
capability-based security model with generation-counter revocation,
capability-gated synchronous IPC, a kernel audit log wired into every
capability operation, and a capability-gated syscall surface (Phase 2).
Phase 3 (User-Space Driver Framework) is in progress: capability-mediated
MMIO/interrupt/port-IO access, real PCIe enumeration, real VT-d IOMMU
bring-up (DMAR discovery through a live translation-enabled root table and
a real per-device DMA domain), a device manager (real classification,
lifecycle state machine, bounded restart-on-crash), `init`/a service
manager (a real ring-3 init process handing off to a service manager via
a genuine capability-gated syscall), and a real ELF64 loader with the
first genuine user-space driver (serial, running from a real compiled
ELF binary, not a hand-built machine-code blob) are done; framebuffer and
PS/2 keyboard drivers and Tier 2 hardware selection remain.
Phases 4 through 8 — storage/filesystem, the agent runtime, driver
synthesis, shell, hardware consolidation — are entirely not started. This
is a genuinely solid, tested foundation; it is not yet an OS with a
filesystem or a real user-space driver running — no claim on this page
should be read as more than what's checked above.

## Next concrete increment

Phase 3 (User-Space Driver Framework), remaining items: framebuffer and
PS/2 keyboard user-space drivers (the ELF-loading path and pattern now
exist, proven on the serial driver — porting these two is now mostly
about MMIO/interrupt capability wiring, not new mechanism). Then Tier 2
physical hardware selection, the last Phase 3 item.
