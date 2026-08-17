# M2 Prompt: Kernel Diagnostics And Panic Path

## Prompt For Antigravity

Implement milestone `M2: Kernel Diagnostics And Panic Path` for Agentic OS.

Inspect `kernel/kernel.c`, `kernel/print.c`, `kernel/interrupts.c`, `kernel/io.c`, `kernel/io.h`, and any existing `serial` or `klog` files before editing.

Goal: make every early kernel failure visible over serial first, framebuffer second.

## Required Changes

- Ensure serial logging initializes before GDT, IDT, interrupts, PMM, VMM, or graphics-dependent diagnostics.
- Provide or harden:
  - `serial_init`
  - `serial_write_char`
  - `serial_write_string`
  - `klog_init`
  - `klog_info`
  - `klog_warn`
  - `klog_error`
  - `panic`
  - `ASSERT`
- `panic` must:
  - disable interrupts
  - write a clear serial error
  - include the panic reason
  - halt reliably
- Exception handlers must log key details over serial:
  - exception name
  - `rip`
  - error code when available
  - `cr2` for page faults
- `printf` must be safer for diagnostics:
  - null format is handled
  - null `%s` prints `(null)`
  - unsigned integer formatting is not routed through signed-only logic
  - zero-length `memmove` does not underflow
- Kernel boot must emit useful checkpoints:
  - `KLOG_INIT`
  - `KERNEL_ENTER`
  - `PMM_INIT_START`
  - `PMM_INIT_DONE`
  - `VMM_INIT_START`
  - `VMM_INIT_DONE`

## Non-Goals

- Do not implement a full logging daemon.
- Do not add persistent logs to disk.
- Do not redesign exception coverage beyond making current handlers reliable unless assigned M6.

## Verification

Run:

```sh
make clean all
make test-boot
```

Review `_evidence/latest/serial.log`.

## Acceptance Criteria

- Boot failures are diagnosable from serial output.
- Kernel checkpoints appear in order.
- Existing framebuffer text output still works as secondary output.
- No new compiler warnings are introduced.
