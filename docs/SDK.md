# Agentic OS Native SDK

Phase 13 deliverable 3 (`docs/ROADMAP.md` §5): "A native SDK/toolchain
targeting the real capability ABI (ADR-004's library, extended, not
replaced) — documented well enough that an app can be built without
reading kernel source."

This document plus the `agentic_sdk` crate (`user_rs/agentic_sdk`) are
that SDK's first real slice. Real, disclosed scope: this covers writing
and building a new ring-3 app; it does not yet cover *installing* one
from a real on-disk package (see `installer.rs`'s own module doc for
that gap) or a UI toolkit (Phase 12 deliverable 4, not started).

## What every app needs

Every `user_rs/*` app in this kernel is a genuine freestanding ELF64
binary, loaded by `kernel_rs/src/elf.rs`'s real loader — never linked
into the kernel image, never sharing its address space. Building one
needs exactly two things:

1. **Three small, per-app build-configuration files** (below) — cargo
   has no mechanism for a shared library crate to inject target/link
   configuration into whatever binary depends on it, so these cannot be
   factored out; every existing app copies them verbatim, changing only
   the linker script's base address.
2. **A dependency on `agentic_sdk`** for the actual code every app
   needs — raw syscalls and COM1 I/O — instead of hand-rolling it.

### The three required files

Copy these from any existing app (`user_rs/serial_driver` is the
simplest reference) into your new crate's own directory:

- **`.cargo/config.toml`** — pins the freestanding target
  (`x86_64-unknown-none`), builds `core`/`compiler_builtins` from
  source (this target's prebuilt `compiler_builtins` lacks the
  `memset`/`memcpy`/`memmove`/`memcmp` intrinsics some apps' code
  ends up needing — see `kernel_common::mem_intrinsics`'s own doc for
  the real bug this once caused), and sets the link flags
  `elf.rs`'s loader requires: `relocation-model=static` (the loader
  only processes `PT_LOAD` segments, never relocation entries — a
  self-relocating/PIE binary would silently be wrong) and
  `--no-dynamic-linker`.
- **`linker.ld`** — a fixed load address (each app gets its own fresh
  address space via `vmm::new_address_space`, so apps never actually
  collide even when they reuse the same base — pick any address that
  doesn't alias the hand-built demos' fixed `0x600000`/`0x700000`, for
  clarity reading a boot log) and page-aligns every output section onto
  its own `PT_LOAD` segment. This alignment is load-bearing, not
  cosmetic: without it, a tiny binary's `.text`(RX) and `.rodata`(R,
  non-executable) can pack into the same 4KB page, and the loader's
  page-by-page mapping lets the second segment's non-executable
  permission silently clobber the first's executable one — a real,
  previously-hit instruction-fetch #PF at the entry point itself.
- **`build.rs`** — one line, `println!("cargo:rerun-if-changed=linker.ld")`.
  Cargo has no dependency edge on a linker script by default (only on
  source files), so editing `linker.ld` alone is silently ignored by an
  incremental build without this.

### Depending on `agentic_sdk`

```toml
[dependencies]
agentic_sdk = { path = "../agentic_sdk" }
```

`agentic_sdk` provides:

- `agentic_sdk::com1::write_str(&str)` / `write_dec_u64(u64)` — real
  COM1 (0x3F8) UART I/O. Requires the app to already hold a granted
  `PortIoRange` capability covering 0x3F8-0x3FF (see `installer.rs` or
  `user_driver.rs` for how that grant happens before ring 3 is ever
  entered) — an ungranted write raises a real #GP, the same hardware
  IOPB boundary with or without this crate.
- `agentic_sdk::syscall::syscall0/1/2/3(num, ...)` — the raw syscall
  ABI (`rax` = number in / return out, up to three arguments in
  `rdi`/`rsi`/`rdx`), with the full caller-saved-register clobber list
  a real bug (found bringing up `virtio_blk_driver`, see
  `agentic_sdk::syscall`'s own doc) requires — `SYSCALL`/`SYSRET` does
  not save/restore registers the way an interrupt does, and the
  kernel's dispatch is free to clobber any of them.
- `agentic_sdk::surface::draw_text(surface_cap, x, y, text, fg, bg)` —
  real PSF1 text rendering into a held `Surface` capability (`SYS_
  SURFACE_DRAW_TEXT`, syscall 14), bounds-checked by the kernel exactly
  like `SYS_SURFACE_FILL` already is. ASCII/Latin-1 glyph indices only
  (PSF1's own 256-glyph table), no UTF-8 decoding.
- `agentic_sdk::text_widget::TextRegion<LINES, COLS>` — Phase 12
  deliverable 4's one real, minimal widget: a fixed-capacity scrollable
  text region (`push_line` shifts the oldest line out once full,
  `render` draws every held line into a surface via `draw_text`). Real,
  disclosed scope: no line-wrapping, no cursor — the single primitive a
  terminal emulator, text editor, or file-manager list view (Phase 13's
  own planned reference apps) all reduce to.

`user_rs/serial_driver` is migrated to `agentic_sdk` as this crate's
own first, real proof that it is a genuine drop-in: same COM1 output,
same syscall behavior, verified against the unmodified regression
suite (`scripts/test-boot.ps1`, `scripts/test-installer.ps1`).

## What isn't here yet

- Higher-level wrappers for specific syscalls (e.g. `SYS_SURFACE_FILL`,
  the audit/introspection queries) — today's apps that need these
  (`window_client_driver`, `agent_demo`) still hand-write their own
  thin wrapper on top of `syscall2`/`syscall3`; folding the common ones
  into `agentic_sdk` itself is real, small, separate follow-up work.
- A way to install an app from a real on-disk package rather than an
  in-kernel `include_bytes!` — blocked on `object_store.rs`'s own real,
  disclosed "one file only" limitation (see `installer.rs`'s module
  doc).
- Any UI-toolkit bindings (Phase 12 deliverable 4 doesn't exist yet).
