//! Real, pure, hardware-independent PCI driver-matching logic — an
//! experimental improvement, built and verified in ISOLATION (real
//! `cargo test` coverage in `host_tests/`, zero wiring into `kernel_rs`'s
//! actual boot path) before any integration decision, per the explicit
//! discipline this was asked to follow: test novel ideas standalone
//! first, only plan integration once they demonstrably help.
//!
//! **What this replaces, if ever integrated:** `kernel_rs/src/main.rs`
//! today calls each driver's own `spawn_if_present(&pci_devices)`
//! sequentially — `virtio_blk::spawn_if_present(...)`,
//! `virtio_net::spawn_if_present(...)`, `ahci::spawn_if_present(...)`,
//! `nvme::spawn_if_present(...)`, `e1000::spawn_if_present(...)` — five
//! near-identical `find_X` functions, each doing its own
//! `.iter().find(...)` over the full device list. This module replaces
//! all five `find_X` functions with ONE real, table-driven matcher —
//! adding a sixth driver in the future means adding one table row, not
//! writing a new near-duplicate `find_X` function.
//!
//! **A real, genuine limitation of TODAY's code this fixes, found while
//! building this, not the reason it was built:** every existing
//! `find_X` uses `.find(...)`, which stops at the FIRST matching
//! device anywhere in the list — if a machine ever has TWO NVMe
//! controllers, only the first would ever get a driver spawned; the
//! second would be silently ignored, with no log line, no error,
//! nothing. `match_all` below has no such limitation: every device in
//! the input is independently matched against the table.
//!
//! Zero heap, matching this crate's existing `#![no_std]`-without-alloc
//! discipline: `match_all` writes into a caller-provided output slice
//! (the same bounded-capacity pattern `kernel_rs::introspect`'s own
//! syscall-facing snapshot functions already use) rather than
//! allocating a `Vec`.

/// The identity fields any driver-matching rule can check — mirrors
/// `kernel_rs::pci::PciDevice`'s own real fields exactly (this module
/// can't depend on kernel_rs, so it defines its own equivalent rather
/// than importing one), so converting a real `PciDevice` into a
/// `PciId` at the call site is a trivial field-for-field copy, not a
/// translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PciId {
    pub vendor: u16,
    pub device: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
}

/// How a driver table entry claims a device — either a specific
/// vendor/device ID pair (`virtio_blk`, `virtio_net`, `e1000` all
/// match this way today) or a device CLASS (`nvme` already matches
/// this way, by necessity — see `kernel_rs/src/nvme.rs`'s own doc on
/// why vendor/device isn't reliable for NVMe).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchRule {
    VendorDevice(u16, u16),
    Class(u8, u8, u8),
}

impl MatchRule {
    pub fn matches(&self, id: &PciId) -> bool {
        match self {
            MatchRule::VendorDevice(v, d) => id.vendor == *v && id.device == *d,
            MatchRule::Class(c, s, p) => id.class == *c && id.subclass == *s && id.prog_if == *p,
        }
    }
}

/// One driver table row. `handler` is generic on purpose: this module
/// has no idea what a "spawn function" looks like in `kernel_rs`'s own
/// type system (a bare `fn()`, a closure, an enum discriminant to
/// switch on) — it only needs to be `Copy` so `match_all` can hand a
/// copy back to the caller for each match, and `PartialEq` so
/// `host_tests` can assert on it directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriverEntry<T: Copy> {
    pub name: &'static str,
    pub rule: MatchRule,
    pub handler: T,
}

/// For every device in `devices`, finds the FIRST table row (in table
/// order) whose rule matches, and writes `(device, handler)` into
/// `out`. Returns the number of matches written — real, bounded
/// behavior: stops once `out` is full rather than silently dropping or
/// panicking, the same "no unbounded growth, no silent truncation
/// without a return value saying so" discipline
/// `kernel_rs::introspect`'s own snapshot functions already follow.
///
/// A device matching NO table row is skipped, not an error — same as
/// today's `spawn_if_present` functions logging "nothing to spawn" and
/// moving on, not halting.
pub fn match_all<T: Copy>(
    devices: &[PciId],
    table: &[DriverEntry<T>],
    out: &mut [(PciId, T)],
) -> usize {
    let mut count = 0;
    for dev in devices {
        if count >= out.len() {
            break;
        }
        for entry in table {
            if entry.rule.matches(dev) {
                out[count] = (*dev, entry.handler);
                count += 1;
                break; // first matching rule wins for this device
            }
        }
    }
    count
}
