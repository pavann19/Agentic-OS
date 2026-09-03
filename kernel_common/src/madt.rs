//! Real ACPI MADT (Multiple APIC Description Table) entry parsing --
//! Phase 9's first real deliverable (`docs/ROADMAP.md` §5 Phase 9,
//! item 1): "AP bring-up via the real INIT-SIPI-SIPI sequence per the
//! MP/ACPI MADT tables already parsed since Phase 3's ACPI work". They
//! were not, in fact, parsed yet -- `kernel_rs::acpi` found and
//! checksum-validated ACPI tables generically (used for DMAR since
//! Phase 3) but never walked MADT's own entry list. This is that walk.
//!
//! Pure over a raw byte slice on purpose, same discipline as
//! `driver_registry` and `pagetable` in this crate: `kernel_rs::acpi`
//! hands this module the MADT table's bytes (via its own `p2v_pub`
//! physical-memory access), and everything here is host-testable
//! with a synthetic byte buffer, zero QEMU, zero unsafe.
//!
//! MADT layout (ACPI spec 5.2.12): the generic `SDT` header (36 bytes,
//! stripped by the caller -- this module receives only the bytes AFTER
//! the header), then a 4-byte Local APIC Address, a 4-byte Flags field,
//! then a variable-length list of interrupt-controller-structure
//! entries, each starting with a 1-byte Type and a 1-byte Length that
//! covers that entry (including its own 2-byte prefix) -- so a
//! consumer that doesn't recognize a Type can still skip it correctly
//! by trusting Length, which is exactly what real MADT walkers must do
//! (new entry types get added by later ACPI revisions; this module
//! must not choke on one it has never seen).
//!
//! Two entry types matter for Phase 9: **Type 0, Processor Local APIC**
//! (the classic 8-bit APIC ID this project's QEMU `q35` target uses,
//! since it never asks for more than 255 processors) and **Type 9,
//! Processor Local x2APIC** (32-bit APIC ID, for systems with more than
//! 255 logical processors or that otherwise report this way) -- both
//! decoded into the same `CpuEntry` shape so callers don't need to care
//! which one a given firmware used.

/// One CPU MADT reported, in caller-agnostic form regardless of
/// whether the firmware used a Type 0 (8-bit ID) or Type 9 (32-bit ID)
/// entry to report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuEntry {
    /// The real local APIC ID this processor answers to -- what a real
    /// INIT-SIPI-SIPI sequence (Phase 9's next real increment) targets
    /// via the LAPIC's ICR, not a bootstrap-assigned index.
    pub apic_id: u32,
    /// ACPI's own processor UID for this entry (distinct from
    /// `apic_id` -- the two numbering spaces are independent per spec,
    /// kept separate here rather than assumed equal).
    pub processor_uid: u32,
    /// Bit 0 of the entry's real Flags field: the processor is usable
    /// NOW. A processor reported but not enabled (and not "online
    /// capable", the bit-1 case some firmwares use for hot-add
    /// processors not yet present) must never receive a real
    /// INIT-SIPI-SIPI -- there may be no silicon there to receive it.
    pub enabled: bool,
}

const TYPE_LOCAL_APIC: u8 = 0;
const TYPE_LOCAL_X2APIC: u8 = 9;

const LOCAL_APIC_ENTRY_LEN: u8 = 8;
const LOCAL_X2APIC_ENTRY_LEN: u8 = 16;

const FLAGS_ENABLED: u32 = 1 << 0;

/// Walks the MADT entry list (the bytes AFTER the 8-byte Local APIC
/// Address + Flags header this function also skips for the caller --
/// pass the full post-SDT-header MADT body), writing every real CPU
/// entry found (Type 0 or Type 9) into `out`. Returns the number
/// written -- same bounded-capacity, no-panic-no-silent-overrun
/// discipline `driver_registry::match_all` already established in this
/// crate: stops once `out` is full rather than growing unboundedly,
/// which matters here because a MADT is attacker/firmware-controlled
/// input this kernel does not get to assume is well-formed.
///
/// Malformed input (a truncated entry, a zero-length entry that would
/// spin forever, a length that would read past `body`'s end) stops the
/// walk cleanly rather than reading out of bounds or looping forever --
/// real defensive parsing, not "well-formed ACPI assumed."
pub fn parse_cpus(body: &[u8], out: &mut [CpuEntry]) -> usize {
    const MADT_SUBHEADER_LEN: usize = 8; // Local APIC Address (4) + Flags (4)
    if body.len() < MADT_SUBHEADER_LEN {
        return 0;
    }
    let mut count = 0;
    let mut i = MADT_SUBHEADER_LEN;
    while i + 2 <= body.len() && count < out.len() {
        let entry_type = body[i];
        let entry_len = body[i + 1];
        if entry_len < 2 {
            break; // malformed -- would never advance, stop rather than spin
        }
        let entry_end = i + entry_len as usize;
        if entry_end > body.len() {
            break; // truncated entry -- stop rather than read out of bounds
        }

        match entry_type {
            TYPE_LOCAL_APIC if entry_len >= LOCAL_APIC_ENTRY_LEN => {
                let processor_uid = body[i + 2] as u32;
                let apic_id = body[i + 3] as u32;
                let flags = u32::from_le_bytes([body[i + 4], body[i + 5], body[i + 6], body[i + 7]]);
                out[count] = CpuEntry {
                    apic_id,
                    processor_uid,
                    enabled: flags & FLAGS_ENABLED != 0,
                };
                count += 1;
            }
            TYPE_LOCAL_X2APIC if entry_len >= LOCAL_X2APIC_ENTRY_LEN => {
                // Layout: type(1) len(1) reserved(2) x2apic_id(4) flags(4) processor_uid(4)
                let apic_id = u32::from_le_bytes([body[i + 4], body[i + 5], body[i + 6], body[i + 7]]);
                let flags = u32::from_le_bytes([body[i + 8], body[i + 9], body[i + 10], body[i + 11]]);
                let processor_uid = u32::from_le_bytes([body[i + 12], body[i + 13], body[i + 14], body[i + 15]]);
                out[count] = CpuEntry {
                    apic_id,
                    processor_uid,
                    enabled: flags & FLAGS_ENABLED != 0,
                };
                count += 1;
            }
            _ => {} // an entry type this module doesn't need (I/O APIC, interrupt source override, ...) -- skip via entry_len, not an error
        }

        i = entry_end;
    }
    count
}
