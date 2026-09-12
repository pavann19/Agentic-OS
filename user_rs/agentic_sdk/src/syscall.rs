//! Real raw syscall ABI — the same asm convention every `user_rs/*`
//! crate's own hand-written wrappers already use against `kernel_rs::
//! syscall`'s dispatch: `rax` carries the syscall number in and the
//! return value out, up to three arguments in `rdi`/`rsi`/`rdx` (this
//! kernel has never yet defined a syscall needing more).
//!
//! `serial_driver`'s own module doc already documented a real bug found
//! bringing up `virtio_blk_driver`: `SYSCALL`/`SYSRET` does not save or
//! restore general-purpose registers the way an interrupt/`iretq` does,
//! and the kernel's own dispatch is a normal `extern "C"` function free
//! to clobber every System V caller-saved register — marking only
//! `rcx`/`r11` (the two the hardware itself repurposes) as clobbered
//! left the compiler free to assume `rdi`/`rsi`/`rdx`/`r8`-`r10` survive
//! a syscall unchanged, true only by accident for call sites that
//! didn't happen to need them afterward. `syscall3` below carries the
//! FULL fix (every argument register `inout`, every caller-saved
//! register `lateout`) as its one canonical implementation, rather than
//! leaving each new crate to rediscover or half-apply it.

#[inline(always)]
pub unsafe fn syscall3(num: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "mov rax, {num}",
        "syscall",
        num = in(reg) num,
        inout("rdi") a0 => _,
        inout("rsi") a1 => _,
        inout("rdx") a2 => _,
        lateout("rax") ret,
        lateout("rcx") _,
        lateout("r8") _,
        lateout("r9") _,
        lateout("r10") _,
        lateout("r11") _,
        options(nostack)
    );
    ret
}

#[inline(always)]
pub unsafe fn syscall0(num: u64) -> u64 {
    syscall3(num, 0, 0, 0)
}

#[inline(always)]
pub unsafe fn syscall1(num: u64, a0: u64) -> u64 {
    syscall3(num, a0, 0, 0)
}

#[inline(always)]
pub unsafe fn syscall2(num: u64, a0: u64, a1: u64) -> u64 {
    syscall3(num, a0, a1, 0)
}
