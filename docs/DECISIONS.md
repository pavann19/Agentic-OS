# Decisions

Short-form rationale for the choices that most shape this codebase. Each
of these has a fuller ADR in `docs/ROADMAP.md` where one exists — this
page exists so the reasoning is readable without going through the whole
roadmap.

## 1. Capabilities, not ACLs or UIDs

Every kernel resource (memory, files, windows, sockets, devices) is
reached through an explicit, typed, per-process capability — never
ambient authority keyed on a process's identity. A process with no
capability to an object cannot even discover the object exists: a
missing grant and a nonexistent object produce the exact same
`NoSuchCapability` error (`kernel_rs/src/capability.rs`), by
construction, not by a separate "permission denied" check layered on
top. The alternative (a Unix-style UID/ACL model) makes "what can this
process touch" a property of who it is; this makes it a property of
what it was explicitly handed — the property an external agent
controlling the OS needs to reason about (see `docs/ROADMAP.md`
ADR-003).

## 2. A generation counter, not a revocation tree

Revoking a capability bumps a `generation: u64` on the underlying
OBJECT; every capability caches the generation it was minted at, and
`resolve()` rejects any capability whose cached generation doesn't
match the object's current one. This makes revocation O(1) regardless
of how many capabilities were derived from the object — no walk over a
derivation tree, no need to even track that tree at runtime. The
tradeoff, stated plainly: revocation is all-or-nothing per object (you
cannot revoke one specific derived capability while leaving siblings
derived from the same object valid) — a real limitation, not
hidden, and acceptable because nothing in this kernel currently needs
finer-grained revocation than "this object is dead now."

## 3. `derive()` rejects, never clamps

Asking to derive a capability with rights broader than the source
capability actually holds is a hard error (`CapError`), not silently
narrowed to whatever subset would have been valid. A caller that
overspecifies rights finds out immediately, at the call site — it
never discovers later that it holds less than it thought it asked for.
Clamping is the more "helpful"-looking default and the one most ACL
systems pick; it was rejected here because silent narrowing is exactly
the kind of place a security-relevant assumption goes stale without
anyone noticing.

## 4. Drivers in user space, not the kernel

Every driver (serial, AHCI, NVMe, xHCI, e1000, virtio-blk/net,
compositor) is an ordinary ring-3 process holding exactly the
capabilities it was granted (MMIO region, IOMMU-backed DMA, port I/O
range), supervised and restarted on crash like any other process. A
driver bug is contained to that driver's own address space and
capability set — it cannot corrupt kernel state it was never granted
access to. The cost is real: every driver I/O path crosses a real
IPC/syscall boundary instead of a function call, and this kernel pays
that cost deliberately (`docs/ROADMAP.md` ADR-001).

## 5. No POSIX compatibility

This OS does not target running existing Linux/Unix binaries, and
`docs/ROADMAP.md` §1.3 states this as a permanent, non-negotiable
scope boundary, not a "not yet." The capability model is fundamentally
incompatible with ambient-authority POSIX semantics (a POSIX process
assumes it can open any path it has permission bits for; a capability
process can only reach what it was explicitly handed) — a compatibility
shim wide enough to paper over that gap would either be fake (silently
granting ambient authority underneath) or so narrow it wouldn't run
real POSIX binaries anyway. Self-hosting groundwork (Milestones 1-2)
targets a real, minimal syscall surface for this OS's own toolchain,
not POSIX emulation.

## 6. Rust `no_std` + UEFI, not a legacy BIOS/multiboot loader

The bootloader targets `x86_64-unknown-uefi` and hands off to a
`no_std` kernel, rather than a legacy BIOS/multiboot path. UEFI is the
real boot path on Tier 2/3 hardware this project actually targets (the
IOMMU/ACPI machinery ADR-006 depends on is UEFI-era infrastructure);
multiboot/BIOS support would be a second, parallel boot path maintained
purely for machines this project has no plan to validate against.
`no_std` Rust removes an entire class of memory-safety bugs at the
language level in exactly the code (kernel, drivers) where a bug has
the largest blast radius — the same reasoning behind ADR-002's language
choice.
