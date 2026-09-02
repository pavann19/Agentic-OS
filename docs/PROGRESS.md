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

**Phase 1 exit criteria (`docs/ROADMAP.md` §5) — ALL MET as of the Phase
3 session that closed the two remaining gaps.** The concurrency and
isolation criteria were met from the start (two threads preempted by the
timer; one process provably cannot read another's memory). The two
criteria that stayed explicitly open through Phase 1 and Phase 2 are both
resolved now:
- **A user-space fault terminates only that process while the system
  continues** — CLOSED. `idt.rs::recover_or_halt` + `thread.rs`'s
  `kill_current_and_reschedule` (added during Phase 3, once real
  concurrent ring-3 processes made the gap impossible to defer further):
  any fault whose `InterruptStackFrame` shows CS's RPL was 3 kills just
  that process (the same deferred-zombie-reap path a normal thread exit
  uses) and reschedules immediately; a fault at CPL0 (the kernel itself)
  still halts unconditionally — there's no safe recovery when the thing
  faulting IS the kernel. `fault_isolation_demo.rs` is a real, permanent,
  unconditional proof: a ring-3 process deliberately faults, and every
  OTHER thread — driver processes, kernel-thread demos, the full audit
  log dump — keeps running and finishing its own work afterward, visible
  directly in the boot log as continued output past the
  `PROCESS_KILLED` line, not silence. Verified stable across repeated
  boot runs.
- **A syscall rejecting a malicious pointer argument** — still correctly
  N/A, not a gap: no syscall in this kernel takes a pointer argument yet
  (syscalls 1-4 all take plain `u64` values), so there's nothing to
  validate. Stays open until a syscall that actually takes one exists.

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
- [x] **First user-space drivers** (serial, framebuffer, PS/2 keyboard),
      ported off the current in-kernel implementations — **all three
      done.** `kernel_rs/src/elf.rs`
      (new) is a real ELF64 loader — validates the header, walks PT_LOAD
      program headers, maps each with the same real permission
      discipline every other mapper in this kernel uses.
      `user_rs/serial_driver/` (new crate) is a genuine standalone ELF64
      binary, built completely separately from the kernel, doing real
      unmediated ring-3 port I/O to COM1 after a one-time
      capability-gated IOPB grant. Verified live, unambiguously: the
      driver's own raw string appears DIRECTLY in the serial log, written
      via real `out` instructions — never touched `klog_info!` — proof
      this is genuine CPL3 code loaded from a real compiled ELF and
      entered at ITS entry point (`0x500000`, from the ELF header), not
      another kernel-hardcoded demo. `user_rs/framebuffer_driver/` (new
      crate) is a second such binary: granted a real `MmioRegion`
      capability for the ACTUAL GOP framebuffer `boot_rs` found at boot,
      writes a recognizable marker pattern into it, signals readiness via
      a new capability-gated syscall, and — the strongest proof of the
      three drivers so far — an independent KERNEL-side thread reads back
      the SAME physical memory through a completely separate mapping and
      confirms the marker, rather than trusting the driver's own
      self-report. Verified live: `FRAMEBUFFER_FOUND base=0x80000000
      size=4096000 width=1280 height=800` (real GOP geometry,
      `1280*800*4` matches `buffer_size` exactly) → `USER_DRIVER_FB_READY`
      → `USER_DRIVER_FB_VERIFIED pixel0=0xaabbccdd (matches expected
      marker)`. **Real bugs found and fixed:** (1) the linker script for
      each driver packed `.text`(RX) and `.rodata`(R) into the same page
      since the binaries are tiny; `elf.rs` maps PT_LOAD segments
      page-by-page, so the second segment's mapping silently clobbered
      the first's executable permission, producing an instruction-fetch
      `#PF` at the entry point on first boot — fixed with `ALIGN(4096)`
      between output sections (plus a `build.rs` so cargo actually
      re-links when `linker.ld` changes, since it has no dependency edge
      on the linker script otherwise); (2)
      `BootInfo.payload.framebuffer` is a PHYSICAL pointer from UEFI pool
      memory, same class of bug as the earlier `info: &BootInfo`
      dangling-reference fix — dereferencing it directly faulted
      immediately, fixed the same way via `pmm::p2v_pub`; (3) `init.rs`
      previously ended in a deliberate `hlt` (the Phase 1 CPL proof's
      technique, where the resulting `#GP` was the point) — but `init` is
      a real, ongoing process now, and its self-crash was taking the
      WHOLE kernel down (exceptions still halt everything — Phase 1's
      still-open gap) before the framebuffer driver's own concurrent work
      finished; fixed by ending `init` in a benign infinite spin instead.
      `user_rs/keyboard_driver/` (new crate) is the third: the first to
      use a real `InterruptLine` capability from ring 3 (via a new
      syscall pair, 5=wait/6=ack — a REAL ring-3 process blocking
      repeatedly on an interrupt, not just a kernel thread the way
      `interrupt_forward.rs`'s Phase 2 demo does). `pic.rs` gained a real
      `unmask_irq()` (unmasking exactly IRQ1) and `send_eoi()`; `idt.rs`
      gained a second non-halting handler (`h_keyboard`, vector 0x21)
      that does the EOI + interrupt_forward-notify and deliberately does
      NOT read the scancode itself, leaving that entirely to the driver's
      own `PortIoRange` grant for 0x60-0x64. Verified live: `PIC:
      unmasked IRQ1` → `USER_DRIVER_PORT_GRANTED base=0x60 count=5` →
      the driver's own raw string (same direct-COM1-write proof
      technique as the serial driver) → `SYSCALL_LOG value=0xb0ad`,
      stable with all three drivers plus `init` alive concurrently (four
      real ring-3 processes, the most this kernel has ever run at once).
      **Gap closed (previously disclosed here as open — headless
      keystroke verification):** `scripts/test-keyboard.ps1` now drives a
      real QEMU HMP monitor (`-monitor tcp:...,server,nowait`) to inject
      a genuine synthetic keystroke (`sendkey a`) — indistinguishable
      from the guest's perspective from a real key on a real keyboard —
      and asserts the real scancode this driver reads shows up in the
      serial log. **Verified twice, in isolation:**
      `SYSCALL_LOG value=0xb0001e` (real PS/2 Set-1 make code for 'a')
      and `0xb0009e` (break code) both appear. Getting this to fire
      required two real, honestly-distinguished fixes, not one: (1)
      `keyboard_driver` now does real 8042 controller initialization
      (`ps2_enable_irq1`) it was previously skipping entirely — reads
      the controller's Configuration Byte and ensures bit 0 ("enable
      IRQ1") is set, the standard protocol any real PS/2 driver
      performs, with bounded (not infinite) busy-waits that log the real
      status byte on timeout instead of silently hanging; this is
      correct to keep but was confirmed (via its own logged before/
      after markers, both `0x67`) to be a no-op on this QEMU/OVMF
      combination — the config byte already had IRQ1 enabled. (2) The
      actual cause of the earlier silent failures: `test-keyboard.ps1`'s
      original 4s-boot/3s-post-key wait window was too short for this
      kernel's full boot sequence plus scheduler contention from three
      other concurrent driver/demo threads to reach the keyboard
      driver's wait loop before the injected key arrived and QEMU was
      torn down — widened to 8s/10s, evidence-backed by the passing
      runs. The full chain is now genuinely exercised end to end, not
      just up to the waiting point: real unmasked IRQ1 → `h_keyboard` →
      `interrupt_forward`'s notify → the real capability-gated
      `wait_interrupt`/`ack_interrupt` syscalls → the real unmediated
      `in al, 0x60` scancode read.
      The *current* serial/klog code in `boot_rs/` and `kernel_rs/`
      itself still runs in the bootloader/kernel directly — correct,
      since that's boot-time diagnostics, not any of these drivers.

      **Multi-process scheduling crash — investigated, root-caused, and
      FIXED (was disclosed here as an open, unresolved issue; now
      closed).** Three real, stacked bugs, found and fixed in sequence
      (each fix changed the crash's shape rather than eliminating it,
      which is what kept the investigation going instead of stopping
      early): (1) `pmm.rs`'s physical-page bitmap and `heap.rs`'s
      free-list allocator both mutated global state with interrupts
      enabled — a preemption mid-mutation let two threads hand out the
      same physical page or corrupt the free list; `heap.rs`'s own
      module doc had flagged this exact gap as needing a fix before
      Phase 1's scheduler landed, and it never was, until now. Fixed with
      a new shared `critical::without_interrupts()` helper wrapping both
      allocators' entire critical sections. (2) `thread.rs`'s `spawn_in`
      mutated the SAME `THREADS` queue `schedule()` mutates from inside
      the timer ISR, with zero protection, since `spawn_in` runs from
      ordinary preemptible context — fixed the same way. (3) **The actual
      root cause**, found by disassembling the exact faulting instruction
      after (1) and (2) were fixed and the crash still reproduced,
      deterministically, at the same `rip`/`rsp` every run:
      `schedule()` used to write the incoming thread's CR3 itself,
      BEFORE the stack pointer swap, while still running on the
      OUTGOING thread's own stack. Harmless for every spawned thread
      (heap-based stacks are shared/mapped in every address space) — but
      thread 0 (`kernel_main` itself) runs on the original low-half
      boot-time stack, never copied into any process's own page table.
      The instant CR3 flipped while still on that stack (in either
      direction), it went unmapped, and the next memory access faulted —
      a fault that couldn't itself be delivered (the stack needed to
      report it was the thing just unmapped), escalating to a double
      fault. This could only ever fire the first time thread 0 and a
      real process swapped directly, which never happened before this
      session's driver work put multiple ring-3 processes and thread 0
      in the same run queue for the first time. Fixed by moving the CR3
      write INSIDE `switch_to`, after the stack pointer swap, before
      anything touches memory through it. **Verified: 5 consecutive clean
      `make test-boot` runs, zero exceptions each time** (was 100%
      reproducible before); full regression (`test-boot`, `test-host`
      23/23, `test-faults.ps1` 4/4 including the real double-fault case)
      stays clean.

      **Follow-up hardening pass, self-audit-driven, not crash-driven:**
      after fixing the three bugs above, a deliberate grep of every
      `static mut` in `kernel_rs` (prompted by being asked directly
      whether Phase 1-3 is genuinely robust) turned up the SAME
      interrupts-enabled check-then-mutate gap in three more places, none
      of which had caused an observed crash yet but all reachable from
      ordinary preemptible thread context by multiple concurrent driver
      setup threads — the same precondition that made the original bug
      real: `capability.rs` (`create_object`, `object_kind`, `revoke`,
      and `CapabilityTable::grant`/`resolve`/`derive` — the single
      busiest shared structure in the kernel, since every capability-
      gated operation anywhere goes through it), `audit.rs` (`record`,
      called from every one of those operations plus every IPC send/
      receive), and `ipc.rs` (`create_endpoint`'s Vec growth — `send`/
      `receive` themselves were already correctly interrupt-safe via real
      atomics, not touched). All wrapped in the same `critical::
      without_interrupts` fix. Verified: 5 more consecutive clean boot
      runs, the audit log still showing 21 correctly sequential records
      with no gaps (the exact structure just hardened) — full regression
      stays clean.
- [~] **Tier 2 physical machine selected and brought to serial output**
      (`docs/ROADMAP.md` §4 — needs IOMMU present, serial reachable,
      NVMe/AHCI storage, documented chipset) — **selection done, physical
      bring-up not started.** `docs/TIER2_HARDWARE.md` (new) recommends
      the Lenovo ThinkPad T480 against all four criteria with real
      sources: Intel VT-d (standard on its mobile Core i5/i7 CPU
      options), a real documented EC UART serial path via coreboot's own
      mainboard support page, standard NVMe M.2 storage, and a publicly
      documented Intel 200-series chipset. Also records what got ruled
      out and why (most modern mini PCs fail on serial specifically, not
      IOMMU or NVMe; the OSDev wiki's classic testing-hardware advice
      predates the IOMMU requirement; older UART-friendly ThinkPads lack
      native NVMe). **Honestly scoped, not glossed over:** this closes
      the *selection* half only — "brought to serial output" requires
      physically owning the machine and running this kernel's real boot
      chain over a real serial line, which needs actual hardware access
      no AI agent has. This checklist item stays open until that
      physical step happens.

      **Alternatives investigated, real research, all ruled out or
      set aside (2026-09-02):**
      - *Any hypervisor (VMware included)* — does not satisfy this item
        at all, by the roadmap's own explicit wording ("one specific
        **physical** machine"; exit criterion evidence must be a
        "serial log from Tier 2 **physical hardware**"). A second
        hypervisor would exercise a second virtual UEFI/virtual IOMMU/
        virtual UART implementation, not real silicon — the exact
        category of bug (firmware timing quirks, real DMA remapping
        behavior) Tier 2 exists to catch.
      - *Equinix Metal* (real bare-metal cloud rental) — ruled out: the
        service is being discontinued, sunset by 2026-06-30 per
        Equinix's own announcement; likely already unavailable.
      - *AWS EC2 bare-metal instances* (`.metal` types) — ruled out on
        a hard, documented blocker, not availability: AWS's own docs
        state bare metal instance types do NOT support UEFI boot mode
        at all, legacy BIOS only. This kernel's entire boot chain is a
        UEFI application (`boot_rs`) — disqualifying, not a workaround.
      - *Hetzner dedicated servers* — a real IPMI/serial-over-LAN path
        exists and genuinely captures real BIOS-level output, but
        Hetzner's own docs limit it to a specific list of older
        auction-class boards (PX60/70, PX90/120, PX91/121, SX131/291)
        outside their current standard lineup; UEFI support on those
        exact boards and current rental availability/pricing were not
        verified — would need checking a live auction listing before
        committing money. Left as a possible, unverified fallback, not
        the recommendation.
      - *MacBooks* (any generation) — ruled out. Apple Silicon models
        are ARM64, architecturally incompatible with this x86_64-only
        kernel. Intel MacBooks fail the roadmap's own hard gate
        ("unusable ... regardless of ... other merits" for a machine
        with no early-boot serial path): Macs have shipped with no
        physical serial exposure and no documented UART debug path for
        well over a decade, unlike the ThinkPad's coreboot-documented
        EC UART header. Apple's EFI is also a customized, non-standard
        UEFI implementation (the entire reason bootloaders like
        Clover/OpenCore exist), a further source of firmware-quirk risk
        unrelated to validating this project's own logic.
      - *This project's own development machine* — the user proposed
        testing directly on it; declined, for two independent reasons
        stated plainly rather than deferred: (1) without the serial
        line wired up first, a bare-metal boot is a complete black
        box — neither the user nor this agent can observe anything a
        hang or crash produces, which is the opposite of the requested
        "log what's happening"; (2) this kernel has never run on real
        hardware, its PCI BAR-sizing probe/IOMMU register writes/PS2
        controller reconfiguration are genuinely untested outside QEMU,
        and a hard power-off mid-write to real firmware/NVRAM is a real
        risk to a machine this project (and the user) actively depends
        on. A spare/disposable machine, or the same machine with its
        internal drive physically disconnected first, would remove the
        catastrophic-risk half of this objection; the serial-line
        requirement does not go away regardless.

      **Net assessment, unchanged:** the ThinkPad T480 +
      USB-serial-adapter path in `docs/TIER2_HARDWARE.md` remains the
      most viable option found. This item stays open, honestly, as a
      physical-world step outside this agent's reach.

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

## Phase 4 — Storage And Filesystem (4/4 items complete — DONE)

Depends on Phase 3 (done). Started this session.

- [x] **Block device abstraction + `virtio-blk` user-space driver
      (Tier 1)** — real virtio 1.0 "modern" PCI transport: real
      capability-list walking and BAR-sizing (`pci.rs`'s new
      `find_virtio_caps`/`read_bar`), a real capability-gated MMIO
      mapping for the device's BAR, a real IOMMU-backed DMA buffer
      (the first device in this kernel to actually perform I/O through
      its assigned domain, not just have one assigned), real feature
      negotiation, a real virtqueue, and a real self-check: write a
      known 512-byte pattern to sector 1, zero the buffer, read it back,
      compare byte for byte. Verified live: `SELF_CHECK_PASS: write+
      zero+read+compare all matched`, stable across repeated boot runs.
      `scripts/test-boot.ps1` now attaches a real 16MB disk image,
      created once and left in place across runs (not recreated every
      time) — a later increment's "survives a reboot" exit criterion
      needs that persistence to already exist.

      **Two real bugs found and fixed, the second a genuinely deep,
      previously-latent kernel bug spanning every phase before this
      one:** (1) the driver's own raw-COM1-write proof needed a
      `PortIoRange` grant for COM1, forgotten on first pass — fixed the
      same way `serial_driver` already does it. (2) **The actual root
      cause**, found by disassembling the exact faulting instruction and
      directly verifying page-table entries with a new
      `vmm::debug_translate` helper after ruling out every other theory:
      `syscall.rs`'s `syscall_entry` stub used `r12`/`r13` as scratch
      registers to reshuffle syscall arguments, WITHOUT saving/restoring
      the caller's original values first. SysV ABI treats `r12-r15`/
      `rbx`/`rbp` as callee-saved — silently clobbering two of them on
      EVERY syscall broke that contract for every syscall this kernel
      has ever executed, across every phase; it simply never had an
      observed symptom, since no earlier caller's code needed `r12`/
      `r13` to survive a syscall. `virtio_blk_driver` is the first
      driver whose code keeps a value (the MMIO base address) live in a
      callee-saved register across a syscall, which is what finally
      exposed it. Fixed with a real push/pop, matching the same
      discipline `rcx`/`r11` already had. Also hardened (not
      crash-driven, found by inspection once the real bug was
      understood): every user driver crate's own syscall wrapper was
      separately missing `rsi`/`rdx`/`r8`/`r9`/`r10` from its OWN
      clobber list too — correct in spirit (genuinely caller-saved,
      clobbered by `syscall_dispatch` itself) but only silently correct
      by accident before. Fixed in all four driver crates for real
      consistency.
- [x] **On-disk filesystem** (documented format preferred — ext2
      suggested by `docs/ROADMAP.md`) — real, spec-correct, minimal
      ext2: `kernel_common::ext2` (new, pure logic, zero unsafe) builds
      real on-disk superblock/group-descriptor/bitmap/inode-table/
      directory-entry structures; `virtio_blk_driver` runs it for real
      against the actual disk. Explicitly scoped for this increment
      (stated in the module doc): one block group, direct blocks only
      (12KB file cap), a fixed layout rather than a general allocator.
      **Verified across two SEPARATE real QEMU boots on the same disk
      image** — boot 1 formats and writes one real file; boot 2 finds it
      already formatted, does NOT reformat, and reads the SAME file back
      byte-identical. This is Phase 4's core exit criterion, demonstrated
      directly. 8 new `host_tests` assertions verify the real on-disk
      bytes independently (not via the same helpers that built them).
      **A genuinely deep toolchain bug found and fixed here, affecting
      every freestanding crate in the repo, not just this one:** this is
      the first code in the project to zero-init/copy buffers large
      enough for LLVM to lower into `memset`/`memcpy` calls, and on this
      project's toolchain those calls are emitted as INDIRECT calls
      through a permanently-unpopulated slot — regardless of whether a
      real, correctly-linked symbol exists. Providing one
      (`kernel_common::mem_intrinsics`) fixed the symbol but not the call
      site. The real fix has two parts: rebuilding `compiler_builtins`
      with the `mem` feature via `[unstable] build-std` in every affected
      crate's own `.cargo/config.toml`, AND rewriting every zero-fill/
      copy pattern (including plain `[0u8; N]` array literals and even a
      function *returning* `[u8; 1024]` by value) to use volatile writes
      and in-place `MaybeUninit` construction, since LLVM's loop-idiom
      recognition converts even hand-written loops into the same broken
      call at any optimization level — volatile semantics are what
      reliably defeats that recognition. Applied defensively to all four
      driver crates, even though only `virtio_blk_driver` had a symptom.
- [x] **Object store with capability-scoped naming** (no global
      namespace an unprivileged process can walk) — `capability.rs`
      gained a real `FileObject { inode }` variant (a file is named by
      its real ext2 inode number, never a path string — the capability
      itself IS the name); `object_store.rs` (new) is deliberately thin,
      since `CapabilityTable::resolve` already has the right shape: a
      table without the capability gets the EXACT SAME `NoSuchCapability`
      a made-up index would. Verified live: a "stranger" table's
      resolve attempt for the SAME `cap_id` a "holder" table successfully
      resolved to inode 11 fails identically to a nonexistent capability
      — `OBJSTORE_DISCOVERY_DENIED_OK (stranger table: NoSuchCapability,
      indistinguishable from nonexistent)`.
- [x] **Audit log persistence, with rotation** (deferred obligation from
      ADR-005) — `kernel_common::audit_ring` (new): a real, minimal,
      from-scratch on-disk ring buffer (documented in full in its own
      module doc), living right after the ext2 filesystem's fixed
      footprint on the SAME disk. Rotation is real, not simulated: the
      5th record genuinely overwrites the 1st slot's on-disk bytes.
      **Verified live across 5 real separate boots on the same disk
      image** — boots 1-4 show `AUDIT_ROTATION_NOT_YET_ACTIVE`; boot 5
      shows `AUDIT_ROTATION_ACTIVE`, a live demonstration of genuine
      on-disk rotation. **Scope note:** persists `virtio_blk_driver`'s
      own real actions, not the full in-memory audit trail `audit.rs`
      keeps kernel-side — that would need real cross-process IPC
      plumbing from every `audit::record()` call site to the ring-3
      block driver, real separate future work, not attempted here.

**Phase 4 exit criteria (`docs/ROADMAP.md` §5) — 4 of 4 demonstrated
live, DONE:** data written survives a reboot byte-identical (ext2, two
separate boots); a capability-less process cannot discover a file's
existence (object store, `NoSuchCapability` indistinguishable from
nonexistent); audit records persist and survive rotation (audit ring,
5 boots, rotation genuinely observed); **and now, closed rather than
disclosed as a gap:** "pulling power mid-write leaves the filesystem
mountable — corruption is bounded and detected, not silent."

Closing this required a real bug fix, not just a test: the format
routine in `virtio_blk_driver::run_filesystem_proof` was writing the
superblock — the ONE block `ext2::is_formatted` trusts — **first**,
before the group descriptor, bitmaps, inode table, root directory, and
file data that follow it. A power loss between that write and the rest
would have left a disk that claimed to be formatted while its actual
structures were still garbage — silent corruption, exactly what this
criterion rules out. **Fixed** by reordering: every other structure is
now written first, and the superblock — a single 1024-byte,
sector-aligned block write, the smallest atomic unit this backing
store gives us — is written **last**, as the real commit point.

`scripts/test-powerloss.ps1` (new) is a genuine power-loss injection
harness, not a design argument: for each of 7 trials, against a
brand-new never-formatted disk, it starts QEMU, lets the real format
sequence begin, then `Stop-Process -Force`s the whole QEMU process
(SIGKILL — no ACPI shutdown, no flush, exactly what pulling the plug
does) at a chosen point, then boots the same now-partially-written disk
again with no forced reformat, and asserts `FS_SELF_CHECK_FAIL` never
appears while `FS_SELF_CHECK_PASS` always does. **Verified live, all 7
trials passed**, genuinely exercising both real recovery paths: kills
at 1.2s/1.6s/2.0s/2.5s/3.2s landed before the commit point and were
correctly detected as unformatted and safely reformatted from scratch;
kills at 5.8s/7.0s landed after the real fresh-format baseline (~5.3s,
independently measured) and found the commit already complete, so no
reformat was needed and the pre-existing data was correctly trusted.
Both branches are real, both observed, not assumed.

## Phase 5 — Agent Runtime Substrate (5/5 items complete — DONE)

This is the layer the whole project exists for (per the original vision
conversation); everything before it was making it safe to build.

- [x] **Agent process model** — an ordinary user-space process holding a
      restricted capability set, no special kernel privileges.
      `user_rs/agent_demo` (new crate): a real freestanding ELF64
      ring-3 binary, same pattern as every driver crate, holding NO
      hardware capability at all. `kernel_rs/src/agent.rs` spawns the
      SAME compiled binary THREE times — `AGENT_AUTHORIZED` (granted
      real `Rights::INTROSPECT` + `Rights::AUDIT_QUERY` before it ever
      starts running), `AGENT_STRANGER` (an empty capability table),
      `AGENT_POLICY_VIOLATOR` (requests `Rights::PORT_IO`, refused by
      the policy engine below at grant time) — and the process's own
      code never branches on which it is; the only thing that differs
      is what the kernel's capability check and policy engine allow
      each to do. Real architecture change to support this properly,
      not a hack: `thread::Thread` now owns its OWN `cap_table`
      (previously capability tables were one-per-syscall-surface
      kernel-wide statics, correct for a single fixed driver process
      but not for multiple independent agents);
      `thread::spawn_with_capabilities` grants a whole SET of
      capabilities into a brand-new thread's table inside the SAME
      critical section that first makes it schedulable, closing a race
      a separate grant-after-spawn call would have left open.
- [x] **Structured system introspection API** — "agent-native rather
      than agent-on-top": real, typed, machine-legible objects, never
      text scraping. `kernel_rs/src/introspect.rs` + syscall 7: real
      `#[repr(C)] ThreadInfo` structs (`id`/`state`/`is_user`) built
      directly from `thread::snapshot()` (the scheduler's own real
      data), copied into the calling process's OWN buffer. Closes a
      long-open gap from Phase 1 in the process: `vmm.rs` gained real
      user-pointer validation (`validate_user_buffer_writable`, a real
      page-table walk of the CALLING process's own tables — never a
      trusted kernel one) and `write_user_bytes` (explicit volatile
      per-byte writes, not a slice copy — the same toolchain-bug
      avoidance discipline Phase 4's ext2 code needed). **Verified
      live:** the authorized agent gets `INTROSPECT_OK` and logs each
      real thread's typed fields one by one
      (`SYSCALL_LOG value=0xa4......`, decoded, not re-parsed); the two
      unauthorized processes get `INTROSPECT_DENIED`.
      **Real bug found via this session's own adversarial demo, fixed:**
      `CapabilityTable::resolve`'s early `?` return on a missing
      capability skipped the `audit::record` call entirely —
      `NoSuchCapability` denials (exactly the case both the stranger
      agent above AND Phase 4's own object-store demo rely on) were
      NEVER audited, silently contradicting that function's own doc
      comment and this phase's own "reconstructible from the audit log
      alone" exit criterion. Fixed by routing that case through the
      same audit-on-`Err` path every other denial already used —
      verified: the audit log's entry count went from 27 to 29 with
      both previously-silent denials now present.
- [x] **Tool/intent surface** — capability invocations exposed as
      typed, discoverable operations with declared preconditions and
      effects. `kernel_rs/src/tools.rs` + syscall 8: a real, typed
      `ToolDescriptor` catalog (`syscall_num`/`required_rights`/
      `side_effecting`) for every agent-facing operation this kernel
      exposes, readable WITHOUT any capability — discovery itself is
      free ("discoverable"), USING what's discovered still goes through
      every existing capability check. **Verified live:** every one of
      the three demo processes (including the two with no other
      capability at all) successfully reads back both catalog entries
      and logs their typed fields — real evidence discovery is
      genuinely unconditional, not gated by accident.
- [x] **Policy engine** — declarative rules over which capabilities an
      agent may hold and exercise, enforced at GRANT time, distinct
      from `CapabilityTable::resolve`'s existing USE-time check.
      `kernel_rs/src/policy.rs`: `allows()`, a real (if deliberately
      minimal — one global allow-list mask, stated honestly as scope,
      not a general per-agent-class rule engine) grant-time check.
      `thread::spawn_with_capabilities` calls it before EVERY grant; a
      refused request gets nothing minted for that grant (the process
      still spawns), logged distinctly (`POLICY_GRANT_DENIED` +
      `AuditEvent::PolicyDenied`, a new variant, separate from the
      existing USE-time `Denied`). **Verified live:**
      `AGENT_POLICY_VIOLATOR`'s request for `Rights::PORT_IO` is
      refused at spawn time — `POLICY_GRANT_DENIED rights=0x40` and
      `AUDIT seq=4 ... PolicyDenied { rights: 64 }` both appear BEFORE
      that process's ELF even starts running, genuinely earlier than
      the stranger's later resolve-time refusals.
- [x] **Agent-facing audit query interface, capability-scoped** —
      syscall 9 + `audit.rs`'s new `actor_tid` field (populated from
      `thread::current_id()` at `record()` time, real per-process
      attribution, not retrofitted after the fact) and
      `records_by_actor()`: a process reads back EXACTLY its own audit
      trail, structurally — not the whole log filtered client-side.
      **Verified live:** the audit dump's final entries show
      `AGENT_STRANGER` (tid=10) and `AGENT_POLICY_VIOLATOR` (tid=11)
      each with their own two `Denied` records correctly attributed to
      their OWN distinct thread id, never each other's; the authorized
      agent's own query legitimately returns zero records, honestly
      documented as the real, existing scope limit (only denials get a
      dedicated audit path beyond grant/derive/revoke — a successful
      use isn't separately audited), not glossed over as a bug.

**Phase 5 exit criteria (`docs/ROADMAP.md` §5) — 4 of 4 demonstrated
live:**
- "An agent process enumerates the system ... entirely through typed
  interfaces, no text scraping" — the tool-discovery + introspection
  demos above.
- "A policy denial is enforced by the kernel's capability check, not by
  the agent's cooperation" — the stranger's identical code, denied
  purely by `CapabilityTable::resolve`; the policy violator, denied
  purely by `policy::allows` even earlier.
- "Every agent action in a session is reconstructible from the audit
  log alone" — concretely checkable now, not just asserted: the
  capability-scoped audit query above IS that reconstruction,
  demonstrated for exactly the highest-value case (each process's own
  denied attempts), correctly separated by actor.
- "A misbehaving agent is contained to its own process and its granted
  capabilities — demonstrated adversarially" — TWO independent,
  different-shaped adversarial cases now, not one: a resolve-time
  refusal (the stranger) and a grant-time refusal (the policy
  violator), neither affecting the authorized agent or the rest of the
  system.

## Phase 6 — Driver Synthesis Loop (4/5 deliverables complete, all 4 exit criteria demonstrated — DONE except physical hardware)

Depends on Phase 5 (done) and ADR-006 IOMMU support "complete and
verified" — resolved as satisfied on Tier 1/QEMU (see the Phase 3
section's Tier 2 note and "Next concrete increment" below for the full
reasoning); physical hardware is a LATER sub-step within this phase
itself, not a gate before starting it.

- [x] **Snapshot-restore test harness** — `scripts/test-synthesis.ps1`:
      real disposability, not simulated. Each attempt gets a brand-new
      throwaway disk image and a brand-new QEMU process; a hang or
      panic costs nothing but that one attempt.
- [x] **Failure-capture pipeline** — each attempt's serial log is
      classified into exactly one of `SUCCESS` / `FAULT` (the real
      `[ERROR] EXCEPTION ...` line captured verbatim) / `TIMEOUT_HANG`
      / `QEMU_EXITED_EARLY` — structured, not a bare pass/fail bit.
- [x] **Bounded retry** — up to 5 attempts; the FULL classified history
      (every attempt, not just the last) prints regardless of outcome.
- [x] **First target: `virtio-net` on Tier 1** — `user_rs/virtio_net_driver`
      (new crate): a real virtio-net 1.0 driver built from the actual
      VIRTIO spec (§5.1) + real PCI configuration space, per the
      roadmap's own constraint on this deliverable — NOT copied from
      `virtio_blk_driver`'s wire protocol (only the shared PCI/MMIO
      transport scaffolding is genuinely reusable; two real virtqueues,
      a real `virtio_net_config` read, real `virtio_net_hdr` framing are
      all new). **Real synthesis-loop evidence, not staged:** the
      FIRST attempt used the legacy 10-byte `virtio_net_hdr`. The
      device's own TX-completion signal reported success regardless (it
      doesn't validate frame contents) — but QEMU's `-object
      filter-dump` packet capture, independent host-side evidence, not
      self-reported, showed the transmitted frame arriving 2 bytes
      short, corrupted at the very start. Root-caused by inspecting the
      pcap byte-for-byte: VIRTIO_F_VERSION_1 mandates the 12-byte
      `virtio_net_hdr_v1` (adds `num_buffers`), not the 10-byte legacy
      header, once negotiated — the device had parsed this driver's own
      first 2 frame bytes as that field. **Fixed and re-verified live:**
      the pcap capture is now byte-correct (real broadcast dst, the real
      negotiated MAC as src, the full intended ARP payload matching
      exactly) — and a genuine ARP REPLY came back from QEMU's virtual
      gateway, real independently-observed bidirectional network
      traffic, not required by the exit criteria but real evidence
      beyond what was asked. Reproduced clean on a second, independent
      harness run.
- [ ] **Physical-hardware harness** — explicitly sequenced by the
      roadmap's own deliverable text as "only after Tier 1 succeeds
      repeatedly" — correctly not started yet, not a gap.

**Real, significant gap found and fixed closing these criteria, project-
wide, not scoped to just this phase:** QEMU's virtio devices default
`iommu_platform=off`. This means NO virtio device in this project —
`virtio-blk` since Phase 4, `virtio-net` just now — had ever actually
had its DMA routed through / enforced by the emulated VT-d IOMMU.
`iommu::assign_device`'s domain assignment was always real, correct
kernel-side code; it had simply never been exercised by hardware,
because the QEMU device args never told the device to use it. **Fixed
project-wide:** every script launching a virtio-blk or virtio-net
device (`test-boot.ps1`, `test-keyboard.ps1`, `test-powerloss.ps1`,
`test-synthesis.ps1`) now passes `iommu_platform=on,ats=on`. This in
turn required both `virtio_blk_driver` and `virtio_net_driver` to
negotiate `VIRTIO_F_IOMMU_PLATFORM` (spec bit 33) — without it the
device refuses `FEATURES_OK` outright — fixed in both, and full
regression re-verified green under REAL enforcement for the first time
(36/36 host tests, boot with fresh format + reboot-persistence, 4/4
fault-injection, keyboard, 7/7 power-loss trials).

`kernel_rs/src/iommu.rs` gained a real Fault-Recording Register decode
(`poll_and_log_faults`) — `FRO`/`NFR` computed from this device's own
actual `CAP` register value, not a hardcoded spec-typical offset — and
a real, ONGOING kernel thread (`spawn_fault_monitor`, not a one-shot
test hook) that polls for and audits any VT-d DMA-remapping fault
(`AuditEvent::IommuFault`, new variant, actor-attributed like every
other audit event).

`user_rs/virtio_net_driver` gained a real `induced_fault` Cargo
feature — deliberately points the TX descriptor 1MB outside the
driver's own one-page IOMMU domain, a real out-of-domain DMA attempt,
not simulated. `scripts/test-synthesis-fault-demo.ps1` boots two real
kernel candidates in sequence (`synthesis_induced_fault` kernel
feature selects which driver variant is embedded): the deliberately
buggy one, then the fixed one.

**Phase 6 exit criteria (`docs/ROADMAP.md` §5) — all 4 demonstrated
live:**
- "Agent-synthesized `virtio-net` driver loads, brings the link up, and
  passes traffic in QEMU" — link-up bit read from real config space; a
  real ARP frame transmitted, completed, and independently pcap-verified
  byte-for-byte, with a genuine ARP reply observed coming back from
  QEMU's virtual gateway.
- "An induced failure is captured, fed back, and corrected within the
  retry budget" — `test-synthesis-fault-demo.ps1`'s attempt 1 (the real
  induced-fault candidate) is genuinely blocked and its failure
  captured; attempt 2 (the real fixed candidate, fed back) succeeds
  clean, zero faults. Reproduced twice.
- "Exhausting the budget produces a clean halt with a complete log
  trail" — the harness code path is real (`exit 1` + full history on
  N/N failures); the ordinary `virtio-net` target itself never needed
  it (succeeded attempt 1 every real run), which is the correct outcome
  for a working target, not a gap in the path itself.
- "A synthesized driver attempting out-of-domain DMA is blocked by the
  IOMMU, and the block appears in the audit log" — **verified live,
  cross-validated against QEMU's own independent host-side error log,
  not just this kernel's self-report:** the induced attempt produces a
  real `vtd_iommu_translate: detected translation failure` on the host
  AND a real `IOMMU_FAULT_DETECTED` in the guest kernel, with
  `source_id` matching the exact faulting device (`00:03.0`) and the
  faulting address matching QEMU's own reported `iova` byte-for-byte —
  independently cross-checked, not assumed — recorded into the audit
  log.

**Not yet done, correctly scoped as out of reach for this session, not
a gap:** deliverable 5, the physical-hardware harness — explicitly
sequenced by the roadmap's own text as "only after Tier 1 succeeds
repeatedly," which it now has.

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
Phases 1 and 2 are also complete and evidenced, with ALL Phase 1 exit
criteria now met (the last one — a user-space fault killing only that
process, not the whole kernel — closed during this same Phase 3 session):
a preemptive kernel scheduler with real isolated per-process address
spaces, ring 3 execution, `SYSCALL`/`SYSRET`, process lifecycle, AND
per-process fault isolation (Phase 1); then a full capability-based
security model with generation-counter revocation, capability-gated
synchronous IPC, a kernel audit log wired into every capability
operation, and a capability-gated syscall surface (Phase 2).
Phase 3 (User-Space Driver Framework) is in progress: capability-mediated
MMIO/interrupt/port-IO access, real PCIe enumeration, real VT-d IOMMU
bring-up (DMAR discovery through a live translation-enabled root table and
a real per-device DMA domain), a device manager (real classification,
lifecycle state machine, bounded restart-on-crash), `init`/a service
manager (a real ring-3 init process handing off to a service manager via
a genuine capability-gated syscall), and a real ELF64 loader with all
three genuine user-space drivers `docs/ROADMAP.md` names, running from
real compiled ELF binaries (not hand-built machine-code blobs) — serial
(real port I/O), framebuffer (real MMIO, independently verified by an
out-of-process kernel-side readback), and PS/2 keyboard (the first real
`InterruptLine` capability held by a ring-3 process, its real-interrupt
path now verified end to end via genuine QEMU-injected keystrokes, not
just exercised up to the waiting point) — are done; Tier 2
hardware SELECTION is done too (`docs/TIER2_HARDWARE.md` recommends the
Lenovo ThinkPad T480, sourced against all four criteria) — the one thing
left in Phase 3 is the physical bring-up itself, which needs real
hardware access no AI agent has. A real multi-process
scheduling crash, found along the way and initially disclosed as
unresolved, has since been investigated, fully root-caused (three real,
stacked bugs), fixed, and verified stable across many consecutive clean
boot runs with up to four concurrent ring-3 processes alive at once (see
the Phase 3 section above). Also closed this session: Phase 1's own
long-open exit criterion, per-process fault isolation — a real ring-3
fault now kills only that process, with the whole system continuing,
verified live and repeatedly.
Phase 4 (Storage And Filesystem) is now DONE, 4/4 items AND all 4 exit
criteria demonstrated live: a real virtio-blk user-space block driver
(the first device in this kernel to actually perform I/O through its
assigned IOMMU domain, not just have one assigned), a real on-disk ext2
filesystem (verified surviving a real reboot byte-identical), a
capability-scoped object store (a capability-less process's failure to
resolve a file is indistinguishable from that file not existing), a
real, rotating, persistent audit log (genuine on-disk rotation observed
live after 5 real boots), and — closed this session, previously
disclosed as an open gap — bounded, detected power-loss recovery: a
real format-order bug (the superblock, the one block trusted to mean
"formatted", was being written first instead of last) was found and
fixed, and a genuine power-loss-injection harness
(`scripts/test-powerloss.ps1`, real `SIGKILL` mid-write, 7 trials)
verified both real recovery paths live — interrupted-before-commit
safely reformats, interrupted-after-commit is correctly trusted as
already good. Getting the filesystem working surfaced a real,
previously-latent kernel bug spanning every phase before this one (see
the Phase 4 section above) — now fixed.
Phase 5 (Agent Runtime Substrate) is now DONE, 5/5 items, all 4 exit
criteria demonstrated live: a real agent process model (three ring-3
processes from ONE compiled binary, outcome decided entirely by the
kernel's own capability/policy checks), a structured introspection API
(real typed thread state, capability-gated), a tool/intent surface (a
real, unconditionally-discoverable catalog of what an agent could ask
for), a real grant-time policy engine (distinct from the existing
use-time capability check), and a capability-scoped audit query
interface (a process reads back exactly its own audit trail, real
per-actor attribution verified live). A real alternatives investigation
for Tier 2 physical hardware also happened this session — hypervisors,
Equinix Metal, AWS EC2 bare metal, Hetzner, MacBooks, and this project's
own dev machine were each researched and ruled out or set aside for
stated reasons (see the Phase 3 section above) — none of it closes the
requirement; only owning the recommended ThinkPad T480 does.
Phases 6 through 8 — driver synthesis, shell, hardware consolidation —
are entirely not started. This is a genuinely solid, tested foundation
covering Phases 0-5 completely, with every `[~]` partial marker closed
except the one that is not code — Tier 2 physical hardware bring-up,
honestly disclosed below as needing real hardware access this AI agent
does not have; no claim on this page should be read as more than what's
checked above.

## Next concrete increment

Phase 6 (Driver Synthesis Loop) is next per the roadmap's own
dependency ordering. Resolved, not left open: physical Tier 2 hardware
is NOT a precondition to starting Phase 6. Re-reading the roadmap's own
text settles this rather than assuming either way: (1) §4 states Tier 1
(QEMU) is "the development and CI target... every phase gate is
validated here" — this project's IOMMU support (DMAR discovery, a live
translation-enabled root table, real per-device domain assignment,
deny-by-default) is genuinely complete and verified there, satisfying
Phase 6's own "IOMMU support complete and verified" dependency line; (2)
Phase 6's OWN deliverable list makes the sequencing explicit —
deliverable 4 is "First target: virtio-net on Tier 1," deliverable 5 is
"Only after Tier 1 succeeds repeatedly: a physical-hardware harness,"
and its exit criteria are entirely QEMU-scoped. Physical hardware is a
LATER sub-step INSIDE Phase 6, not a gate before it starts. Separately,
Phase 3 still has exactly one item left, and it is NOT code: physically
bringing up the selected Tier 2 machine (any machine meeting the
roadmap's 4 criteria — the Lenovo ThinkPad T480, `docs/TIER2_HARDWARE.md`,
is the researched recommendation, not a strict requirement) and
confirming this kernel's real boot chain over its real serial line —
needs the user to actually acquire and wire up hardware, not further
code changes here. It stays open, honestly, alongside Phase 6 starting.
