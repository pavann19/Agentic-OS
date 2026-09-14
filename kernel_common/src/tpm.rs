//! Pure `#![no_std]` TPM 2.0 TCG event log and measured-boot abstractions.
//!
//! Conforms to the TCG PC Client Platform Firmware Profile Specification:
//!   - PCR[0]: Platform firmware & UEFI initialization.
//!   - PCR[4]: Bootloader stage (`boot_rs` execution).
//!   - PCR[9]: Kernel payload (`kernel.elf` binary execution).
//!
//! Provides in-memory TCG event log recording, PCR extension simulation
//! (`PCR_new = SHA256(PCR_old || EventDigest)`), and integrity attestation.

use crate::crypto::Sha256;

pub const PCR_PLATFORM_FIRMWARE: u32 = 0;
pub const PCR_BOOTLOADER: u32 = 4;
pub const PCR_KERNEL_PAYLOAD: u32 = 9;

pub const EV_POST_CODE: u32 = 0x0000_0001;
pub const EV_SEPARATOR: u32 = 0x0000_0004;
pub const EV_ACTION: u32 = 0x0000_0005;
pub const EV_COMPACT_HASH: u32 = 0x0000_000C;
pub const EV_IPL: u32 = 0x0000_000D; // Initial Program Load (Kernel)

pub const MAX_TPM_EVENTS: usize = 16;

/// A single TCG PC Client TPM 2.0 PCR event record.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TcgEvent {
    pub pcr_index: u32,
    pub event_type: u32,
    pub digest: [u8; 32],
    pub event_data_len: u16,
    pub event_data: [u8; 32],
}

impl TcgEvent {
    pub const fn empty() -> Self {
        Self {
            pcr_index: 0,
            event_type: 0,
            digest: [0u8; 32],
            event_data_len: 0,
            event_data: [0u8; 32],
        }
    }

    pub fn new(pcr_index: u32, event_type: u32, digest: &[u8; 32], data: &[u8]) -> Self {
        let mut event_data = [0u8; 32];
        let copy_len = data.len().min(32);
        event_data[..copy_len].copy_from_slice(&data[..copy_len]);
        Self {
            pcr_index,
            event_type,
            digest: *digest,
            event_data_len: copy_len as u16,
            event_data,
        }
    }
}

/// In-memory TCG measured boot log.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TcgEventLog {
    pub magic: [u8; 8],
    pub count: u32,
    pub events: [TcgEvent; MAX_TPM_EVENTS],
}

pub const TPM_LOG_MAGIC: [u8; 8] = *b"TCGLOG2\0";

impl TcgEventLog {
    pub const fn new() -> Self {
        Self {
            magic: TPM_LOG_MAGIC,
            count: 0,
            events: [TcgEvent::empty(); MAX_TPM_EVENTS],
        }
    }

    /// Records a measurement into the TCG log.
    pub fn record(&mut self, pcr_index: u32, event_type: u32, digest: &[u8; 32], data: &[u8]) -> bool {
        if self.count as usize >= MAX_TPM_EVENTS {
            return false;
        }
        self.events[self.count as usize] = TcgEvent::new(pcr_index, event_type, digest, data);
        self.count += 1;
        true
    }

    /// Reconstructs / validates the simulated PCR value by replaying all log events
    /// for a given PCR index: PCR_final = SHA256( ... SHA256(SHA256(0 || E1) || E2) ... )
    pub fn compute_pcr(&self, pcr_index: u32) -> [u8; 32] {
        let mut pcr = [0u8; 32];
        for i in 0..self.count as usize {
            let event = &self.events[i];
            if event.pcr_index == pcr_index {
                // TCG extend formula: PCR_new = SHA256(PCR_old || event.digest)
                let mut hasher = Sha256::new();
                hasher.update(&pcr);
                hasher.update(&event.digest);
                pcr = hasher.finalize();
            }
        }
        pcr
    }
}
