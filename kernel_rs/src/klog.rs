//! Port of `kernel/klog.c`. Same `[INFO]`/`[WARN]`/`[ERROR]` prefixes and the
//! same `KLOG_INIT` checkpoint string, so `_evidence/latest/serial.log`
//! parsing (human or `make test-boot` grep) doesn't need to change.
//!
//! Unlike the C version, formatting goes through `core::fmt::Write` instead
//! of a hand-rolled `%s/%d/%u/%x` parser — `write!`/`writeln!` give us that
//! for free and it's one of the concrete safety wins ADR-002 was chosen for
//! (no variadic `...` args, no format-string/argument mismatches possible).

use crate::serial::SerialWriter;
use core::fmt::Write;

pub fn init() {
    crate::serial::init();
    crate::serial::write_str("KLOG_INIT\n");
}

pub fn info(args: core::fmt::Arguments) {
    let _ = write!(SerialWriter, "[INFO] ");
    let _ = SerialWriter.write_fmt(args);
    let _ = SerialWriter.write_str("\n");
}

pub fn warn(args: core::fmt::Arguments) {
    let _ = write!(SerialWriter, "[WARN] ");
    let _ = SerialWriter.write_fmt(args);
    let _ = SerialWriter.write_str("\n");
}

pub fn error(args: core::fmt::Arguments) {
    let _ = write!(SerialWriter, "[ERROR] ");
    let _ = SerialWriter.write_fmt(args);
    let _ = SerialWriter.write_str("\n");
}

#[macro_export]
macro_rules! klog_info {
    ($($arg:tt)*) => ($crate::klog::info(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! klog_warn {
    ($($arg:tt)*) => ($crate::klog::warn(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! klog_error {
    ($($arg:tt)*) => ($crate::klog::error(format_args!($($arg)*)));
}

/// Port of `kernel/klog.c:panic`. Disables interrupts, logs, halts forever.
/// Phase 0 still needs this reachable from a `#[panic_handler]` for Rust-level
/// panics (unwraps, bounds checks, etc.) in addition to explicit calls.
pub fn panic(reason: &str) -> ! {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
    error(format_args!("PANIC: {}", reason));
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}
