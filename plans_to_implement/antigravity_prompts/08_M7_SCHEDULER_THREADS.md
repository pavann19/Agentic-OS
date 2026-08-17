# M7 Prompt: Scheduler And Kernel Threads

## Prompt For Antigravity

Implement milestone `M7: Scheduler And Kernel Threads`.

Inspect timer code, interrupt code, heap code, stack allocation, and current kernel main loop before editing.

Goal: run multiple kernel threads safely on a single CPU.

## Required Changes

- Add thread structure:
  - thread id
  - name
  - state
  - priority placeholder
  - kernel stack
  - saved register context
  - CPU time/tick accounting
- Add states:
  - `THREAD_NEW`
  - `THREAD_RUNNABLE`
  - `THREAD_RUNNING`
  - `THREAD_BLOCKED`
  - `THREAD_EXITED`
- Add API:
  - `Thread* thread_create(void (*entry)(void*), void* arg, const char* name)`
  - `void thread_exit(void)`
  - `void yield(void)`
  - `void schedule(void)`
  - `void scheduler_init(void)`
  - `void scheduler_start(void)`
- Implement cooperative scheduling first.
- Add timer-driven preemption only after cooperative switching is stable.
- Add idle thread.
- Add basic synchronization:
  - `spinlock_lock`
  - `spinlock_unlock`
  - interrupt-safe lock variant if needed
- Keep scheduler single-core.

## Non-Goals

- Do not implement user processes.
- Do not implement SMP.
- Do not implement advanced priorities or real-time scheduling.

## Verification

Run:

```sh
make clean all
make test-boot
```

Add a test mode or boot diagnostic that starts two kernel threads printing counters through serial.

## Acceptance Criteria

- Two kernel threads run and yield without corrupting serial output.
- Idle thread runs when no other thread is runnable.
- Timer/preemption is either working with evidence or explicitly deferred.
- Kernel remains bootable without test mode enabled.
