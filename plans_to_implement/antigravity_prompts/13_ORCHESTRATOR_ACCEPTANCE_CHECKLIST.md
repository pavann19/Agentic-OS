# Orchestrator Acceptance Checklist

Use this checklist after every Antigravity implementation stage.

## Diff Review

- Confirm changed files match the assigned stage.
- Reject unrelated refactors.
- Reject feature work that bypasses unresolved boot, PMM, VMM, or diagnostics issues.
- Confirm public interfaces match the stage prompt.
- Confirm docs were updated only where they clarify current truth or accepted future work.

## Evidence Review

- Confirm `_evidence/latest/` exists.
- Confirm serial logs were regenerated during this stage.
- Confirm required checkpoints are present and ordered correctly.
- Confirm logs are not stale by checking timestamps or rerunning the command.
- Confirm skipped tests are listed in `known-failures.md` or the final report.

## Command Review

Run the commands yourself:

```sh
docker compose run --rm os-build make clean all
docker compose run --rm os-build make test-boot
docker compose run --rm os-build make test-host
```

For later stages, also run:

```sh
docker compose run --rm os-build make test-faults
docker compose run --rm os-build make test-integration
docker compose run --rm os-build make test-release
```

## Rejection Rules

Reject the stage if:

- Build fails.
- QEMU boot evidence is missing.
- Serial checkpoints are missing.
- The agent claims success without command output.
- The implementation uses host OS behavior at runtime.
- Kernel AI/model logic was added.
- Memory safety issues are hidden under UI changes.
- The stage expands into future milestones without evidence.

## Acceptance Rules

Accept the stage only when:

- The diff is scoped.
- Required commands pass or failures are explicitly accepted by you.
- Evidence files are current.
- Known failures are honest.
- The next milestone can start without guessing what changed.
