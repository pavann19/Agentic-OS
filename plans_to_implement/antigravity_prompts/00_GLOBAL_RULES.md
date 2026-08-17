# Global Rules For Antigravity 3.1 Pro

You are implementing a real `x86_64 UEFI` operating system, not a hobby demo. Treat correctness, evidence, and boot reproducibility as mandatory.

## Role

You are the implementation worker. The human is the orchestrator. Do not self-certify completion. Provide evidence that the orchestrator can verify independently.

## Non-Negotiable Rules

- Inspect the current repository before editing. Do not assume the prompt reflects the latest code.
- Keep changes scoped to the assigned milestone.
- Do not implement AI/model logic inside the kernel.
- Do not hide broken boot behavior behind UI changes.
- Do not add features that depend on Windows, Linux, WSL, or a host subsystem at OS runtime.
- Docker/Linux build tools are allowed only for building and testing the OS artifact.
- Every boot-related change must preserve or improve serial evidence.
- Every stage must leave the repo buildable unless the prompt explicitly says it is an intermediate branch.

## Required Evidence

Create or update `_evidence/latest/` during verification. Include whichever files apply:

- `build.log`: full clean build output.
- `serial.log`: QEMU serial output.
- `host-tests.log`: host-side unit/integration test output.
- `qemu-exit-code.txt`: QEMU exit code when available.
- `known-failures.md`: honest list of remaining failures or skipped checks.
- `changed-files.txt`: files modified by the stage.

## Required Verification Commands

Run these whenever applicable:

```sh
make clean all
make test-boot
make test-host
```

If using Docker:

```sh
docker compose run --rm os-build make clean all
docker compose run --rm os-build make test-boot
docker compose run --rm os-build make test-host
```

## Required Final Report

End with:

- Files changed.
- Public interfaces changed.
- Commands run and pass/fail result.
- Evidence file paths.
- Known failures.
- What the orchestrator should verify manually.

## Engineering Standard

- Prefer small, auditable steps over broad rewrites.
- Fail closed: if boot metadata, memory maps, ELF files, or syscalls are invalid, stop with a serial-visible error.
- Kernel faults must be visible over serial.
- User-space faults must eventually kill the offending process, not crash the kernel.
- Keep kernel APIs explicit; avoid hidden assumptions such as "pages are probably contiguous."
