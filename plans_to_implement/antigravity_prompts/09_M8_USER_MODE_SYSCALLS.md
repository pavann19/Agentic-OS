# M8 Prompt: User Mode And Syscalls

## Prompt For Antigravity

Implement milestone `M8: User Mode And Syscalls`.

Inspect GDT/TSS, IDT, VMM address spaces, scheduler, ELF loader code if any, and kernel logging before editing.

Goal: execute isolated ring 3 user programs and provide a minimal syscall ABI.

## Required Changes

- Add user/kernel privilege transition:
  - ring 3 code and data segments or long-mode equivalent setup
  - TSS `rsp0`
  - syscall/sysret or interrupt-based syscall entry
- Define syscall ABI:
  - `rax` contains syscall number
  - standard registers contain arguments
  - `rax` contains return value
  - negative return values indicate errors
- Add initial syscall numbers:
  - `SYS_EXIT=1`
  - `SYS_WRITE=2`
  - `SYS_YIELD=3`
  - `SYS_GET_TIME=4`
  - `SYS_DEBUG_LOG=5` if useful
- Add process abstraction:
  - PID
  - address space
  - main thread
  - lifecycle state
  - exit code
- Add user ELF loader:
  - validate ELF64
  - map segments into user address space
  - set user stack
  - enter user mode at ELF entry
- Validate all user pointers before kernel access.
- Page faults in user space should terminate the process when possible, not panic the kernel.

## Non-Goals

- Do not implement filesystem-backed `/bin/init` unless M9 is also assigned.
- It is acceptable to embed a tiny user test program for this stage only, but document it as temporary.
- Do not add agent runtime yet.

## Verification

Run:

```sh
make clean all
make test-boot
```

Add a user-mode smoke test that writes to serial via syscall.

## Acceptance Criteria

- Kernel enters user mode.
- User program calls `write` syscall and output appears in serial log.
- Invalid syscall returns an error.
- Invalid user pointer is rejected.
- User page fault does not crash the whole kernel when recovery path exists.
