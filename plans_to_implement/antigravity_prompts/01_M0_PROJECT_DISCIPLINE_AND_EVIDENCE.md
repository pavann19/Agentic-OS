# M0 Prompt: Project Discipline And Evidence

## Prompt For Antigravity

Implement milestone `M0: Project Discipline And Evidence` for Agentic OS.

First inspect the repository, especially `Makefile`, `Dockerfile`, `docker-compose.yml`, `README.md`, `docs/`, `_evidence/`, `boot/`, `kernel/`, and `include/`.

Goal: make the project understandable, buildable, and verifiable from a clean environment.

## Required Changes

- Ensure `README.md` clearly states target architecture: `x86_64`, UEFI, freestanding C kernel, QEMU/OVMF first, bare metal later.
- Ensure `docs/ARCHITECTURE.md` explains the boot sequence, kernel/user-space boundaries, memory policy, interrupt policy, and agentic layer placement.
- Ensure `docs/ROADMAP.md` lists milestones `M0-M11` with acceptance gates.
- Ensure `docs/ANTIGRAVITY_TASK_CONTRACT.md` defines agent rules, evidence expectations, and orchestrator verification.
- Ensure `_evidence/README.md` explains generated evidence files.
- Ensure Docker compose runs a meaningful default command, not an idle command.
- Ensure the `Makefile` exposes `all`, `image`, `run-qemu`, `test-boot`, `test-host`, and `clean`.
- Ensure `make test-boot` captures serial output into `_evidence/latest/serial.log` and asserts at least one kernel checkpoint.

## Non-Goals

- Do not redesign PMM/VMM in this stage.
- Do not add scheduler, filesystem, user mode, or shell.
- Do not change boot behavior unless needed for testability.

## Verification

Run:

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

## Acceptance Criteria

- A fresh build creates `os-image.img`.
- `_evidence/latest/serial.log` exists after `make test-boot`.
- The serial log contains deterministic boot checkpoint text.
- Documentation is accurate to the current implementation and clearly marks future work.
- Final report includes changed files, commands run, evidence paths, and known failures.
