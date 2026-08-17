# M11 Prompt: Automated Testing And Release Candidate

## Prompt For Antigravity

Implement milestone `M11: Automated Testing And Release Candidate`.

Inspect all previous test targets, `_evidence/`, QEMU commands, host tests, docs, and current boot logs before editing.

Goal: prove the OS works repeatedly through automated build, boot, fault, integration, and evidence checks.

## Required Changes

- Add complete test targets:
  - `make test-build`
  - `make test-host`
  - `make test-boot`
  - `make test-faults`
  - `make test-integration`
  - `make test-release`
- Host tests should cover:
  - PMM bitmap/range logic
  - ELF parser
  - string/memory primitives
  - VFS path parser
  - syscall table validation
  - shell parser
- QEMU boot tests must capture serial output and assert checkpoints in order.
- Fault tests must cover:
  - null dereference
  - invalid syscall
  - invalid user pointer
  - user page fault
  - allocation exhaustion
- Integration tests must cover:
  - boot to init
  - shell command script
  - VFS file read
  - two user processes or kernel threads depending on current milestone status
- Store artifacts under `_evidence/latest/`:
  - `build.log`
  - `serial.log`
  - `faults-serial.log`
  - `integration-serial.log`
  - `host-tests.log`
  - `qemu-exit-code.txt`
  - optional screenshot
  - `release-summary.md`
- Update docs to explain how to reproduce a release candidate build.

## Non-Goals

- Do not add new product features in this milestone.
- Do not weaken tests to pass.
- Do not accept manual-only verification as release evidence.

## Verification

Run:

```sh
make test-release
```

If using Docker:

```sh
docker compose run --rm os-build make test-release
```

## Acceptance Criteria

- Fresh build succeeds.
- Headless QEMU boot reaches init/shell checkpoint.
- Fault tests produce expected serial evidence.
- Host tests pass.
- Release summary honestly lists supported features, unsupported features, known bugs, and next risks.
- The orchestrator can reproduce the result using one documented command.
