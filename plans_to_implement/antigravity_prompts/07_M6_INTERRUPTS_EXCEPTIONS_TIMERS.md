# M6 Prompt: Interrupts, Exceptions, And Timers

## Prompt For Antigravity

Implement milestone `M6: Interrupts, Exceptions, And Timers`.

Inspect `kernel/interrupts.c`, `kernel/idt.c`, `kernel/gdt.c`, `kernel/pic.c`, `kernel/keyboard.c`, `kernel/io.c`, and logging code before editing.

Goal: build a stable interrupt foundation with full exception visibility, safe keyboard input, and timer ticks.

## Required Changes

- Add handlers for CPU exception vectors `0-31`.
- Exceptions must log vector name, vector number, `rip`, error code when present, and `cr2` for page faults.
- Add TSS support and IST stack for double fault and critical exceptions.
- Keep legacy PIC as compatibility fallback.
- Add timer support:
  - PIT fallback is acceptable for first implementation.
  - APIC timer may be added after ACPI/MADT parsing is reliable.
- Timer interrupt must increment a monotonic tick counter.
- Add API:
  - `uint64_t timer_ticks(void)`
  - `void timer_init(void)`
  - `void sleep_ms(uint64_t ms)` may busy-wait until scheduler exists.
- Move keyboard rendering out of IRQ context:
  - IRQ handler reads scancode and acknowledges interrupt.
  - Scancode is pushed into a small ring buffer.
  - Main loop or later scheduler consumes input.
- Add input API:
  - `int keyboard_try_read_scancode(uint8_t* out)`
  - optional translated character API if already clean.

## Non-Goals

- Do not implement full APIC/IOAPIC unless assigned and tested.
- Do not implement scheduler in this stage.
- Do not perform framebuffer drawing inside IRQ handlers.

## Verification

Run:

```sh
make clean all
make test-boot
```

Add serial checkpoints:

- `EXCEPTIONS_INIT_DONE`
- `TIMER_INIT_DONE`
- `KEYBOARD_QUEUE_INIT_DONE`

## Acceptance Criteria

- Timer ticks are visible in serial or diagnostic output.
- Keyboard IRQ does not call rendering or full buffer swap directly.
- Exceptions have serial-visible diagnostics.
- Double fault path uses a dedicated safe stack.
