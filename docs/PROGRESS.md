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

## Phase 0 — Foundation Correctness (1/9 items complete, 2 in progress)

Depends on: ADR sign-off (done — ADR-001/002/003/006 accepted, ADR-002
specifically extended mid-build to cover the bootloader too, see
`NATIVE_BUILD.md`). Blocks everything after it.

- [x] **Toolchain decision (ADR-002) executed** — Rust, both bootloader and
      kernel. Native, Docker-free: `rustup` (nightly, GNU ABI) + `QEMU` via
      `winget`, no MSVC/gnu-efi/mtools. See `NATIVE_BUILD.md`.
- [~] **Bootloader ported to Rust** — first slice only. `boot_rs/` boots in
      QEMU/OVMF, verified this session: `BOOT_START` → `BOOT_RS_HELLO_OK` on
      serial, reproducible from `make clean`. Does **not** yet load
      `kernel.elf`, read the font, build the memory map, or construct
      `BootInfo` — `boot/main.c`'s ELF-loading logic is not ported. See
      `NATIVE_BUILD.md` "Next integration step".
- [~] **Kernel ported to Rust (boot-critical slice)** — `kernel_rs/`
      compiles to a correct static, non-PIE ELF (verified via `objdump`:
      `EXEC_P`, zero dynamic relocations, entry matches `linker.ld`). Ports
      `BootInfo` validation, serial, klog, panic handling 1:1 from the C
      reference. **Not boot-tested** — nothing loads it yet, since the
      bootloader can't load ELF files yet (see above). The `KERNEL_ENTER`
      checkpoint has not been observed since the crate restructure to a
      `bin` crate; do not treat it as current. See `PHASE0_PROGRESS.md`.
- [ ] All 32 CPU exception vectors handled, TSS/IST double-fault path —
      not started in Rust. (C reference only handles 3 of 32, no
      IST/TSS at all — see the original repo audit.)
- [ ] VMM redesign — higher-half kernel, per-page permissions (NX,
      read-only text, user/supervisor), real `unmap`, TLB invalidation,
      null-guard, no blanket identity map — not started in Rust.
- [ ] Kernel heap (`kmalloc`/`kfree`/`kcalloc`, aligned allocation) — not
      started.
- [ ] Local APIC timer — not started.
- [ ] Deferred interrupt work (IRQ handlers capture-and-return, no
      rendering/parsing/scheduling in interrupt context) — not started;
      not yet applicable since no IRQ handling exists in `kernel_rs/` yet.
- [ ] Host test harness with real assertions (`make test-host`) — still a
      stub (`"No host unit tests exist yet."`). Not upgraded this session.
- [ ] Fault-injection test suite (`make test-faults`) — does not exist.

**Phase 0 exit criteria (from `docs/ROADMAP.md` §5)** — none met yet: no
null-guard fault, no NX/read-only enforcement, no higher-half kernel, no
timer, no interrupt-context discipline to verify, no host or fault-injection
test suite. The one exit criterion partially addressable today (does the
kernel run from the higher half) is blocked on the VMM item above, which
hasn't started.

---

## Phase 1 — Execution Model — Not started

Depends on Phase 0 (incomplete). Threads, per-process address spaces, ring 3,
`SYSCALL`/`SYSRET`, process lifecycle — none of this exists yet, in C or
Rust.

## Phase 2 — Capability And IPC Substrate — Not started

Depends on Phase 1. Capability table, grant/attenuate/revoke, IPC, the
kernel-level audit log (ADR-005), interrupt forwarding to user space — none
of this exists yet. This is the phase where the OS becomes agent-native
rather than a conventional kernel; nothing in the current boot-slice touches
it.

## Phase 3 — User-Space Driver Framework — Not started

Depends on Phase 2. IOMMU bring-up (ADR-006, hard gate), PCIe enumeration,
device manager, `init`, first user-space drivers (serial/framebuffer/PS2),
Tier 2 physical hardware selection — none started. Notably: the *current*
serial/klog code in both `boot_rs/` and `kernel_rs/` runs in the bootloader
and kernel directly (correct for where they are now — this is boot-time
diagnostics, not a driver), not as a user-space driver — that move happens
here, later.

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

Two independently-verified but not-yet-connected pieces exist: a Rust UEFI
bootloader that boots and proves the native toolchain end-to-end, and a
Rust kernel binary that compiles correctly but has never run. Everything
from "the bootloader actually loads the kernel" onward — which is most of
Phase 0, and all of Phases 1 through 8 — is either in progress or not
started. This is early, real, evidenced progress on the foundation; it is
not yet a bootable, functioning kernel, and no claim on this page should be
read as more than what's checked above.

## Next concrete increment

Per `NATIVE_BUILD.md`: port `boot/main.c`'s ELF-loading logic into
`boot_rs/` (needs `EFI_BOOT_SERVICES`, `EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`,
`EFI_LOADED_IMAGE_PROTOCOL` bindings not modeled yet). That closes the gap
between the two `[~]` items above and produces the first real
`KERNEL_ENTER` checkpoint from the native Rust pipeline — the milestone
that actually completes item 2 and 3 above and unblocks the rest of Phase 0.
