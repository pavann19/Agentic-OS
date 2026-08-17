# M10 Prompt: Minimal UX And Agentic Preparation

## Prompt For Antigravity

Implement milestone `M10: Minimal UX And Agentic Preparation`.

Inspect keyboard input queue, framebuffer/text rendering, scheduler, VFS, process model, and logging before editing.

Goal: provide a useful minimal shell and define safe boundaries for future agentic services.

## Required Changes

- Keep the current desktop as diagnostic UI, not the product UI.
- Add shell command loop in user space if user mode exists; otherwise add a temporary kernel shell with a clear TODO to migrate.
- Add shell commands:
  - `mem`
  - `ps`
  - `ls`
  - `cat`
  - `log`
  - `reboot` if reboot path exists
  - `help`
- Shell must consume keyboard input through the input queue, not direct IRQ rendering.
- Add basic command parser with bounded buffers.
- Add diagnostics APIs needed by shell:
  - memory stats
  - process/thread list
  - log tail or serial-only fallback
  - VFS directory read if available
- Define future agentic service boundaries in docs and placeholder interfaces:
  - `agentd`
  - `policyd`
  - `indexd`
  - `auditd`
- Define audit log record shape:
  - timestamp/tick
  - actor
  - action
  - target
  - decision
  - result

## Non-Goals

- Do not integrate any model provider.
- Do not put agent planning inside the kernel.
- Do not build a polished GUI before the shell and diagnostics are reliable.

## Verification

Run:

```sh
make clean all
make test-boot
```

Add scripted or serial-driven shell tests if practical.

## Acceptance Criteria

- User can type commands.
- `mem` reports PMM/heap stats.
- `ps` reports kernel threads/processes.
- `ls` and `cat` read through VFS when M9 is complete.
- Agentic service docs clearly keep AI behavior in user space.
