//! Phase 9.5a (`docs/ROADMAP.md` §5) — pure, host-testable logic for
//! real process supervision: identifying WHICH PCI device a crashed
//! driver process belongs to (a bus/device/function triple packed into
//! one `u32`, the same shape VT-d's own Source ID field already uses,
//! per `audit.rs::AuditEvent::IommuFault`'s own precedent), and the
//! restart-vs-quarantine decision `device_manager.rs::report_crash`
//! already made informally, now factored out so it can be host-tested
//! directly rather than only exercised by booting a real kernel.
//!
//! This module has NO knowledge of threads, interrupts, PCI hardware,
//! or the audit log — `kernel_rs::device_manager`/`kernel_rs::supervisor`
//! wrap it with all of that real state; this is just the arithmetic.

/// Packs a PCI bus/device/function triple into one `u32` — bus in bits
/// [23:16], device in [15:8], function in [7:0]. Matches the field
/// widths VT-d's own real Source ID register uses (bus is a full byte;
/// device is 5 bits, function 3, but packing device/function as two
/// full bytes here is simpler and still round-trips exactly, since a
/// real PCI device/function are always < 32/< 8 and this format has
/// room to spare).
pub fn pack_bdf(bus: u8, device: u8, function: u8) -> u32 {
    ((bus as u32) << 16) | ((device as u32) << 8) | (function as u32)
}

/// Inverse of `pack_bdf` — real round-trip, not an approximation.
pub fn unpack_bdf(packed: u32) -> (u8, u8, u8) {
    (((packed >> 16) & 0xFF) as u8, ((packed >> 8) & 0xFF) as u8, (packed & 0xFF) as u8)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Restart is allowed — `attempt` is the 1-based restart attempt
    /// number this decision represents (the FIRST restart after a
    /// fresh device is attempt 1, matching `device_manager.rs`'s own
    /// pre-existing log line shape, "restarting (attempt N/MAX)").
    Restart { attempt: u32 },
    /// The device has exhausted its restart budget — quarantine it
    /// (leave it permanently failed) rather than restart again.
    Quarantine,
}

/// The real restart-vs-quarantine decision, given how many times this
/// device has ALREADY been restarted (`prior_restart_count`) and the
/// real, bounded budget (`max_restarts`). Pure function: no side
/// effects, no I/O — `device_manager.rs::report_crash` is the real
/// stateful wrapper around this that actually mutates a device's
/// recorded `restart_count` and logs the outcome.
pub fn decide_restart(prior_restart_count: u32, max_restarts: u32) -> RestartDecision {
    if prior_restart_count >= max_restarts {
        RestartDecision::Quarantine
    } else {
        RestartDecision::Restart { attempt: prior_restart_count + 1 }
    }
}
