# Feature Status

What actually gets exercised by the automated regression suite
(`scripts/test-integration.ps1`, run before every commit that touches the
subsystem it covers) versus what has only ever been demonstrated once, by
hand, in a single session. Cross-reference: `docs/PROGRESS.md` for the
phase-by-phase build history, `docs/ROADMAP.md` for the ADRs behind each
decision.

Everything on this page has been run in QEMU (TCG software emulation).
Nothing on this page has been run on physical hardware yet — that gap is
tracked separately (`docs/ROADMAP.md` §4, Tier 2/3) and applies uniformly
to every row below, so it isn't repeated per row.

Status legend:
- **Tested** — covered by a named script in the regression suite, runs on
  every relevant change, asserts real evidence (not just "didn't crash").
- **Tested (narrow)** — has a regression test, but the test's own scope is
  disclosed as smaller than the subsystem's full surface.
- **Experimental** — real, working code, demonstrated live at least once,
  but not wired into the regression suite — a regression here would not
  be caught automatically.
- **Known gap** — a specific, currently-open defect or missing piece,
  not a general disclaimer.

| Subsystem | Status | Regression coverage | Notes |
|---|---|---|---|
| Boot (UEFI → kernel handoff) | Tested | `test-boot` | Three-checkpoint chain (`BOOT_START` → `EXIT_BOOT_SERVICES_OK` → `KERNEL_ENTER`), also the CI smoke test. |
| Capability/IPC substrate | Tested | `test-faults`, `test-synthesis` | Core to every other subsystem's own tests, not just its own. |
| Fault isolation / driver crash-restart | Tested | `test-faults` | |
| Multi-core (SMP) bring-up | Tested (narrow) | `test-boot` (bring-up only) | Race-soak explicitly skips itself on fewer than 3 real cores — see `SMP_RACE_SOAK_SKIPPED` in the boot log; not exercised on every CI runner. |
| Power-loss / crash safety (ext2) | Tested | `test-powerloss` | Superblock-last write ordering; explicitly does NOT claim mid-superblock-write safety (disclosed in `virtio_blk_driver`'s own doc). |
| Storage: virtio-blk | Tested | `test-boot`, `test-e1000`, others | |
| Storage: AHCI | Tested | `test-ahci` | |
| Storage: NVMe | Tested | `test-nvme` | |
| Filesystem: ext2 (flat file, Phase 4) | Tested | `test-boot`, `test-powerloss` | |
| Filesystem: ext2 directories/path resolution (Self-Hosting Milestone 2) | Tested | `test-self-hosting` | |
| USB: xHCI | Tested | `test-xhci` | |
| USB HID (mouse/keyboard over USB) | Known gap | none | Configure Endpoint bug still open (Phase 11); PS/2 is the only tested input path. |
| Network: e1000 | Tested | `test-e1000` | Fixed this session — was colliding with the Agent-Download feature over a shared hardcoded inode; both now use independent storage. |
| Network: virtio-net / TCP stack | Tested | `test-synthesis`, `test-tcp-two-instance` | |
| Display server / compositor | Tested | `test-compositor`, `test-compositor-crash` | |
| Input routing (PS/2 → focused window) | Tested | `test-input-routing` | Fixed this session — a placeholder window was reachable by `FocusWindow`/`InjectKey` without ever being registered for real delivery, so it silently dropped keys behind a false-positive success response. |
| Mouse | Tested | `test-mouse` | |
| Reference apps: terminal, text editor, file manager, net client | Tested | `test-terminal`, `test-text-editor`, `test-file-manager` | |
| App manifest / installer | Tested | `test-manifest`, `test-installer` | |
| Shell | Tested | `test-shell` | |
| Live Agent Bridge (COM2 external-agent control) | Tested | `test-agent-bridge` | Also driven live, ad hoc, over the real TCP/serial protocol this session (not just the scripted test) — that's how the input-routing bug above was found. |
| Agent-driven internet download | Tested | `test-agent-download` | |
| Self-hosting groundwork (spawn, wait, exit, real path-based exec) | Tested | `test-self-hosting` | `Rights::EXEC`-gated; adversarial denial case included. |
| Multi-user / update integrity / hardware root of trust | Experimental | none in the 26-suite battery | Phase 14 deliverables exist and were demonstrated at landing time; not re-run automatically since. |
| Agent runtime substrate (Phase 5 capability demo) | Experimental | none in the 26-suite battery | Runs every boot as part of `main.rs`, but nothing asserts its output — would silently rot if broken. |
| Release build reproducibility | Tested | `test-release` | Hash-compares real release artifacts, not the dev build. |
| AMD chipset support | Known gap | none | Not yet started — every driver/IOMMU path here has only ever been validated against QEMU's default (Intel-chipset-modeled) `q35` machine and Intel-vendor virtual PCI IDs. Scoped as its own task, after the items above. |
| Parser robustness (ELF loader, PCI config space, ext2 on-disk structures, HTTP response parsing, Live Agent Bridge wire framing) | Known gap | none | No fuzzing harness exists yet for any of these. Every one of them parses either untrusted-in-principle input (network bytes, a downloaded HTTP response) or on-disk state that could be corrupted (a stale/foreign-formatted disk, as this session's `test-e1000` root cause happened to demonstrate is a real failure mode). Scoped as its own task. |
| Real hardware (any Tier 2/3 machine) | Known gap | none | Everything above is QEMU-only. This is the single largest gap in the project — see `docs/ROADMAP.md` §4. |

## How to keep this page honest

Update it in the same commit as whatever changes a row — same discipline
`docs/PROGRESS.md` already follows. A subsystem doesn't move from
Experimental to Tested because it was demonstrated once more by hand; it
moves when a named script exercising it is actually wired into
`scripts/test-integration.ps1`'s suite list.
