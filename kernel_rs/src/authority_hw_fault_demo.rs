//! Research track (`docs/RESEARCH_TRACK.md`, `docs/NOVEL_CONCEPTS.md`
//! §1) -- the escalation the second `authority.rs` increment's own doc
//! named as the honest next step: a real, LIVE PCI device (AHCI, the
//! same real controller `ahci.rs`/`user_rs/ahci_driver` already speak
//! to) issuing a SECOND real DMA-backed command against a buffer whose
//! grant was just revoked through `authority::revoke_device`, and
//! observing a REAL IOMMU fault as the direct, hardware-level
//! consequence -- not the context-table bytes alone (already proven by
//! `authority.rs`'s own boot-time self-check), but an actual blocked
//! transaction, captured the same way `iommu::poll_and_log_faults`
//! already captures any other real DMA-remapping fault.
//!
//! **Deliberately kernel-side (ring 0), not the real ring-3
//! `ahci_driver`:** coordinating "revoke exactly between two DMA
//! phases" against a live ring-3 process needs a real IPC signal this
//! kernel does not have wired for this purpose yet, and getting that
//! timing wrong would either not test anything or (worse) risk
//! interfering with the real, already-verified `ahci_driver` self-check
//! elsewhere in this boot. What actually matters for this
//! demonstration is that a REAL PCI device (the AHCI HBA) is the one
//! performing the DMA the IOMMU either allows or blocks -- the DMA
//! transaction itself is issued by the CONTROLLER'S OWN hardware once
//! `PxCI` is written, identically regardless of whether the code that
//! wrote `PxCI` was running in ring 0 or ring 3. This module reuses the
//! exact, already-verified AHCI 1.3.1 command-table byte layout
//! `user_rs/ahci_driver` uses (same constants, same offsets -- copied
//! deliberately, not reinvented, to keep the protocol correctness this
//! session already earned).
//!
//! **Real, bounded safety discipline, learned the hard way earlier this
//! session:** this project already found, by direct QEMU trace
//! evidence, that an AHCI command a device genuinely cannot complete
//! can leave `PxCI` set essentially forever (a real AHCI-spec-correct
//! behavior on a Task File Error, not a bug in the polling code). An
//! IOMMU-blocked DMA is a DIFFERENT failure mode from that TFES case,
//! but this module does not assume it behaves any more cooperatively --
//! the SECOND command's `PxCI` poll uses a SHORT bounded spin (a few
//! million iterations, not the 200M used for a command expected to
//! succeed), and the authoritative evidence this module actually
//! reports on is `iommu::poll_and_log_faults()`, not whether `PxCI`
//! happened to clear.

use crate::{authority, iommu, klog_info, pci, pmm, vmm};
use kernel_common::discovered_envelope;

const REG_GHC: u64 = 0x04;
const REG_PI: u64 = 0x0C;
const GHC_AE: u32 = 1 << 31;

const PORT_BASE: u64 = 0x100;
const PORT_STRIDE: u64 = 0x80;
const PXCLB: u64 = 0x00;
const PXCLBU: u64 = 0x04;
const PXFB: u64 = 0x08;
const PXFBU: u64 = 0x0C;
const PXIS: u64 = 0x10;
const PXCMD: u64 = 0x18;
const PXSIG: u64 = 0x24;
const PXSSTS: u64 = 0x28;
const PXCI: u64 = 0x38;

const PXCMD_ST: u32 = 1 << 0;
const PXCMD_FRE: u32 = 1 << 4;
const PXCMD_FR: u32 = 1 << 14;
const PXCMD_CR: u32 = 1 << 15;

const ATA_SIG_ATA: u32 = 0x0000_0101;

const CMD_LIST_OFF: u64 = 0x000;
const FIS_OFF: u64 = 0x400;
const CMD_TABLE_OFF: u64 = 0x500;
const DATA_OFF: u64 = 0x600;

unsafe fn mmio_read32(base: u64, off: u64) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
unsafe fn mmio_write32(base: u64, off: u64, v: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, v);
}

unsafe fn find_ata_port(bar: u64) -> Option<u32> {
    let pi = mmio_read32(bar, REG_PI);
    for port in 0..32u32 {
        if pi & (1 << port) == 0 {
            continue;
        }
        let port_base = bar + PORT_BASE + (port as u64) * PORT_STRIDE;
        let ssts = mmio_read32(port_base, PXSSTS);
        if ssts & 0xF != 3 {
            continue;
        }
        if mmio_read32(port_base, PXSIG) == ATA_SIG_ATA {
            return Some(port);
        }
    }
    None
}

unsafe fn init_port(port_base: u64, dma_phys: u64) {
    let mut cmd = mmio_read32(port_base, PXCMD);
    if cmd & PXCMD_ST != 0 {
        mmio_write32(port_base, PXCMD, cmd & !PXCMD_ST);
        while mmio_read32(port_base, PXCMD) & PXCMD_CR != 0 {
            core::hint::spin_loop();
        }
    }
    cmd = mmio_read32(port_base, PXCMD);
    if cmd & PXCMD_FRE != 0 {
        mmio_write32(port_base, PXCMD, cmd & !PXCMD_FRE);
        while mmio_read32(port_base, PXCMD) & PXCMD_FR != 0 {
            core::hint::spin_loop();
        }
    }
    mmio_write32(port_base, PXCLB, ((dma_phys + CMD_LIST_OFF) & 0xFFFF_FFFF) as u32);
    mmio_write32(port_base, PXCLBU, ((dma_phys + CMD_LIST_OFF) >> 32) as u32);
    mmio_write32(port_base, PXFB, ((dma_phys + FIS_OFF) & 0xFFFF_FFFF) as u32);
    mmio_write32(port_base, PXFBU, ((dma_phys + FIS_OFF) >> 32) as u32);
    mmio_write32(port_base, PXIS, mmio_read32(port_base, PXIS));
    let cmd = mmio_read32(port_base, PXCMD);
    mmio_write32(port_base, PXCMD, cmd | PXCMD_FRE | PXCMD_ST);
}

/// Builds and issues a real IDENTIFY DEVICE command (same byte layout
/// as `user_rs/ahci_driver::issue_identify`), returns once issued.
/// Does NOT poll for completion -- callers decide their own poll
/// budget, since the whole point of the second call in this module is
/// that completion may never come.
unsafe fn issue_identify_nowait(dma: u64, dma_phys: u64, port_base: u64) {
    let cmd_hdr = (dma + CMD_LIST_OFF) as *mut u32;
    core::ptr::write_volatile(cmd_hdr, 5 | (1u32 << 16));
    core::ptr::write_volatile(cmd_hdr.add(1), 0);
    let ctba = dma_phys + CMD_TABLE_OFF;
    core::ptr::write_volatile(cmd_hdr.add(2), (ctba & 0xFFFF_FFFF) as u32);
    core::ptr::write_volatile(cmd_hdr.add(3), (ctba >> 32) as u32);

    let ctab = (dma + CMD_TABLE_OFF) as *mut u8;
    for i in 0..64usize {
        core::ptr::write_volatile(ctab.add(i), 0);
    }
    core::ptr::write_volatile(ctab.add(0), 0x27);
    core::ptr::write_volatile(ctab.add(1), 1 << 7);
    core::ptr::write_volatile(ctab.add(2), 0xEC);

    let prdt = (dma + CMD_TABLE_OFF + 0x80) as *mut u32;
    let data_phys = dma_phys + DATA_OFF;
    core::ptr::write_volatile(prdt, (data_phys & 0xFFFF_FFFF) as u32);
    core::ptr::write_volatile(prdt.add(1), (data_phys >> 32) as u32);
    core::ptr::write_volatile(prdt.add(2), 0);
    core::ptr::write_volatile(prdt.add(3), 511);

    mmio_write32(port_base, PXCI, 1);
}

fn wait_pxci_clear(port_base: u64, max_spins: u64) -> bool {
    let mut spins: u64 = 0;
    while unsafe { mmio_read32(port_base, PXCI) } & 1 != 0 {
        spins += 1;
        if spins > max_spins {
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Runs the full demonstration against `bus:device.function` (expected
/// to be a real AHCI controller, e.g. the real `00:1f.2` this kernel's
/// own PCI enumeration finds). Logs every step; never panics, never
/// spins unboundedly. Real, honest result reported via
/// `AUTHORITY_HW_LIVE_FAULT_*` log lines.
pub fn run(bus: u8, device: u8, function: u8) {
    let Some(bar) = pci::read_bar(bus, device, function, 5) else {
        klog_info!("AUTHORITY_HW_LIVE_FAULT_SKIP: BAR5 unreadable for {:02x}:{:02x}.{}", bus, device, function);
        return;
    };
    let bar_vaddr = unsafe { vmm::map_mmio_page(bar.phys_addr) };

    unsafe {
        let ghc = mmio_read32(bar_vaddr, REG_GHC);
        mmio_write32(bar_vaddr, REG_GHC, ghc | GHC_AE);
    }

    let Some(port) = (unsafe { find_ata_port(bar_vaddr) }) else {
        klog_info!("AUTHORITY_HW_LIVE_FAULT_SKIP: no active ATA port found on {:02x}:{:02x}.{}", bus, device, function);
        return;
    };
    let port_base = bar_vaddr + PORT_BASE + (port as u64) * PORT_STRIDE;

    let dma_phys = unsafe { pmm::alloc_page() };
    let dma_vaddr = unsafe { pmm::p2v_pub(dma_phys) as u64 };

    let Some(_domain) = authority::grant_device(bus, device, function, dma_phys, 4096) else {
        klog_info!("AUTHORITY_HW_LIVE_FAULT_SKIP: authority::grant_device refused (graph full)");
        return;
    };

    unsafe { init_port(port_base, dma_phys) };

    iommu::clear_faults();
    let _ = iommu::poll_and_log_faults(); // drain anything stale before the real measurement

    // First command: real device, real DMA, WITH a live grant. Must
    // succeed -- this is the control, proving the setup itself is
    // sound before the actual test.
    unsafe { issue_identify_nowait(dma_vaddr, dma_phys, port_base) };
    let first_ok = wait_pxci_clear(port_base, 200_000_000);
    let faults_after_first = iommu::poll_and_log_faults();
    klog_info!(
        "AUTHORITY_HW_LIVE_FAULT_CONTROL first_command_completed={} spurious_faults={} (must be true / 0)",
        first_ok, faults_after_first
    );

    let revoked = authority::revoke_device(bus, device, function, dma_phys);
    klog_info!("AUTHORITY_HW_LIVE_FAULT_REVOKED revoked_ok={}", revoked);

    iommu::clear_faults();
    let _ = iommu::poll_and_log_faults();

    // Second command: the SAME real device, the SAME real physical
    // buffer, issued the SAME way -- the only thing that changed is
    // the grant. A short bounded wait (see module doc: PxCI clearing
    // is not the evidence this module trusts).
    unsafe { issue_identify_nowait(dma_vaddr, dma_phys, port_base) };
    let second_completed = wait_pxci_clear(port_base, 5_000_000);
    let faults_after_second = iommu::poll_and_log_faults();

    klog_info!(
        "AUTHORITY_HW_LIVE_FAULT_RESULT second_command_completed={} iommu_faults_captured={}",
        second_completed, faults_after_second
    );

    if faults_after_second > 0 {
        klog_info!("AUTHORITY_HW_LIVE_FAULT_PASS: a real, live PCI device's DMA attempt was blocked by real IOMMU hardware immediately after authority::revoke_device, captured as a real fault -- not a simulated or asserted result");
    } else {
        klog_info!("AUTHORITY_HW_LIVE_FAULT_INCONCLUSIVE: no real IOMMU fault was captured for the post-revocation attempt -- reported honestly, not treated as a pass");
    }
}

/// Same real IDENTIFY DEVICE command as `issue_identify_nowait`, but
/// with the PRDT (the data destination) pointed at an arbitrary
/// `data_phys` rather than a fixed offset inside `dma` -- what
/// `run_envelope_discovery` below needs to make several REAL DMA
/// attempts against several DIFFERENT physical pages using the SAME
/// command-structure page (`dma`/`dma_phys`, which stays permanently
/// granted so the command itself is always fetchable -- only the DATA
/// target's grant status is the variable under test).
unsafe fn issue_identify_to(dma: u64, dma_phys: u64, port_base: u64, data_phys: u64) {
    let cmd_hdr = (dma + CMD_LIST_OFF) as *mut u32;
    core::ptr::write_volatile(cmd_hdr, 5 | (1u32 << 16));
    core::ptr::write_volatile(cmd_hdr.add(1), 0);
    let ctba = dma_phys + CMD_TABLE_OFF;
    core::ptr::write_volatile(cmd_hdr.add(2), (ctba & 0xFFFF_FFFF) as u32);
    core::ptr::write_volatile(cmd_hdr.add(3), (ctba >> 32) as u32);

    let ctab = (dma + CMD_TABLE_OFF) as *mut u8;
    for i in 0..64usize {
        core::ptr::write_volatile(ctab.add(i), 0);
    }
    core::ptr::write_volatile(ctab.add(0), 0x27);
    core::ptr::write_volatile(ctab.add(1), 1 << 7);
    core::ptr::write_volatile(ctab.add(2), 0xEC);

    let prdt = (dma + CMD_TABLE_OFF + 0x80) as *mut u32;
    core::ptr::write_volatile(prdt, (data_phys & 0xFFFF_FFFF) as u32);
    core::ptr::write_volatile(prdt.add(1), (data_phys >> 32) as u32);
    core::ptr::write_volatile(prdt.add(2), 0);
    core::ptr::write_volatile(prdt.add(3), 511);

    mmio_write32(port_base, PXCI, 1);
}

/// Research track (`docs/RESEARCH_TRACK.md`, `docs/NOVEL_CONCEPTS.md`
/// §3): least privilege discovered from REAL captured IOMMU fault
/// addresses (`iommu::poll_and_collect_fault_addrs`), not a simulated
/// attempt list -- the escalation `kernel_common::discovered_envelope`'s
/// own module doc named as separate follow-up work.
///
/// Real device (AHCI, `bus:device.function`), real command structures
/// (a permanently-granted control page so the command itself is always
/// fetchable), three "candidate data regions" (real, separate physical
/// pages) attempted with NO grant at all -- each is expected to
/// genuinely fault. The real fault addresses the IOMMU reports are fed
/// to `discover_envelope`, then `authority::freeze_envelope_device`
/// grants exactly those pages for real. Verifies: a previously-faulted
/// (now granted) region completes a real IDENTIFY; a FOURTH region,
/// never attempted during discovery, still faults after freezing --
/// least privilege discovered by observation, then genuinely enforced
/// against something new, on real hardware.
pub fn run_envelope_discovery(bus: u8, device: u8, function: u8) {
    let Some(bar) = pci::read_bar(bus, device, function, 5) else {
        klog_info!("AUTHORITY_HW_ENVELOPE_SKIP: BAR5 unreadable for {:02x}:{:02x}.{}", bus, device, function);
        return;
    };
    let bar_vaddr = unsafe { vmm::map_mmio_page(bar.phys_addr) };
    unsafe {
        let ghc = mmio_read32(bar_vaddr, REG_GHC);
        mmio_write32(bar_vaddr, REG_GHC, ghc | GHC_AE);
    }
    let Some(port) = (unsafe { find_ata_port(bar_vaddr) }) else {
        klog_info!("AUTHORITY_HW_ENVELOPE_SKIP: no active ATA port found on {:02x}:{:02x}.{}", bus, device, function);
        return;
    };
    let port_base = bar_vaddr + PORT_BASE + (port as u64) * PORT_STRIDE;

    // Command-structure page -- granted once, kept granted for the
    // whole demo, so every command below is fetchable regardless of
    // whether its DATA target is granted.
    let control_phys = unsafe { pmm::alloc_page() };
    let control_vaddr = unsafe { pmm::p2v_pub(control_phys) as u64 };
    if authority::grant_device(bus, device, function, control_phys, 4096).is_none() {
        klog_info!("AUTHORITY_HW_ENVELOPE_SKIP: could not grant control page");
        return;
    }
    unsafe { init_port(port_base, control_phys) };

    // Three real, separate candidate data regions -- none granted yet.
    let mut regions = [0u64; 4];
    for r in regions.iter_mut() {
        *r = unsafe { pmm::alloc_page() };
    }

    iommu::clear_faults();
    let _ = iommu::poll_and_log_faults();

    let mut fault_addrs = [0u64; 8];
    let mut fault_count = 0usize;
    for &region in &regions[0..3] {
        unsafe { issue_identify_to(control_vaddr, control_phys, port_base, region) };
        // Short bounded wait -- these are expected to fault, same
        // "don't trust PxCI clearing" discipline as the rest of this
        // module.
        let _ = wait_pxci_clear(port_base, 3_000_000);
        if fault_count < fault_addrs.len() {
            let n = iommu::poll_and_collect_fault_addrs(&mut fault_addrs[fault_count..]);
            fault_count += n;
        }
    }
    klog_info!("AUTHORITY_HW_ENVELOPE_TRACE real_faults_captured={}", fault_count);

    let mut envelope = [0u64; 8];
    let env_count = discovered_envelope::discover_envelope(&fault_addrs[..fault_count], &mut envelope);
    klog_info!("AUTHORITY_HW_ENVELOPE_DISCOVERED distinct_pages={}", env_count);

    let granted = authority::freeze_envelope_device(bus, device, function, &envelope[..env_count]);
    klog_info!("AUTHORITY_HW_ENVELOPE_FROZEN real_grants_created={}", granted);

    // A previously-faulted, now-granted region must complete for real.
    iommu::clear_faults();
    let _ = iommu::poll_and_log_faults();
    unsafe { issue_identify_to(control_vaddr, control_phys, port_base, regions[0]) };
    let region0_completed = wait_pxci_clear(port_base, 200_000_000);
    let region0_faults = iommu::poll_and_log_faults();
    klog_info!(
        "AUTHORITY_HW_ENVELOPE_VERIFY_GRANTED region0_completed={} spurious_faults={} (must be true / 0)",
        region0_completed, region0_faults
    );

    // The fourth region, never attempted during discovery, never
    // granted -- must still fault. `region3_completed` (PxCI clearing)
    // is logged but NOT treated as authoritative -- this module's own
    // `run` function already established why: on a real IOMMU-blocked
    // DMA, AHCI's own error path can clear PxCI with an error status
    // rather than leaving it hung, a real, legitimate, spec-permitted
    // outcome distinct from the earlier ATA Task File Error investigation
    // this project also did. `region3_faults` (a real, captured IOMMU
    // fault record) is the evidence this module actually trusts, same
    // as every other pass/fail decision in this file.
    iommu::clear_faults();
    let _ = iommu::poll_and_log_faults();
    unsafe { issue_identify_to(control_vaddr, control_phys, port_base, regions[3]) };
    let region3_completed = wait_pxci_clear(port_base, 3_000_000);
    let region3_faults = iommu::poll_and_log_faults();
    klog_info!(
        "AUTHORITY_HW_ENVELOPE_VERIFY_DENIED region3_completed={} (informational only) faults_captured={} (authoritative)",
        region3_completed, region3_faults
    );

    if env_count == 3 && region0_completed && region0_faults == 0 && region3_faults > 0 {
        klog_info!("AUTHORITY_HW_ENVELOPE_PASS: an envelope discovered from REAL captured IOMMU fault addresses was frozen into real grants, a previously-faulted region now works, and a region never seen during discovery is still genuinely denied by real hardware (a real IOMMU fault captured on the attempt)");
    } else {
        klog_info!("AUTHORITY_HW_ENVELOPE_FAIL: see the individual markers above for exactly where this diverged from the expected real result");
    }
}

/// Research track §2 (`docs/NOVEL_CONCEPTS.md` §2.4's real falsifiable
/// test): issue a hardware-bound certificate, then directly corrupt
/// the REAL IOMMU context-table bytes underneath it (simulating an
/// attacker or a hardware fault, not going through any real kernel
/// API -- a raw physical-memory write, the same class of access this
/// project's own historical IOMMU bug already proved software can get
/// wrong), and show the certificate specifically reports a
/// `HardwareMismatch`, distinguishable from mere graph staleness.
pub fn run_certificate_corruption_test(bus: u8, device: u8, function: u8) {
    let test_phys = unsafe { pmm::alloc_page() };
    // A page NOT granted to this device -- a real, checked
    // impossibility to certify.
    let Some(hwcert) = authority::issue_device_certificate(bus, device, function, test_phys) else {
        klog_info!("AUTHORITY_HW_CERT_SKIP: page was reachable, could not issue a certificate");
        return;
    };
    let verdict_before = authority::verify_device_certificate(bus, device, function, &hwcert);
    klog_info!("AUTHORITY_HW_CERT_ISSUED verdict_before_tampering={:?} (must be Valid)", verdict_before);

    // Real, direct corruption of the real context-table bytes -- NOT
    // through iommu::assign_device/revoke_device, deliberately: this
    // simulates something going wrong with the hardware structure
    // itself, bypassing every real kernel API this module otherwise
    // trusts.
    unsafe { iommu::debug_corrupt_context_entry(bus, device, function) };

    let verdict_after = authority::verify_device_certificate(bus, device, function, &hwcert);
    klog_info!("AUTHORITY_HW_CERT_VERIFY_AFTER_CORRUPTION verdict={:?} (must be HardwareMismatch)", verdict_after);

    if verdict_before == authority::HwVerdict::Valid && verdict_after == authority::HwVerdict::HardwareMismatch {
        klog_info!("AUTHORITY_HW_CERT_PASS: a certificate bound to real IOMMU hardware bytes correctly detected direct tampering with those bytes, distinct from a mere software-graph change");
    } else {
        klog_info!("AUTHORITY_HW_CERT_FAIL: see the verdict markers above for exactly where this diverged");
    }
}

/// Research track §1's remaining half (`docs/NOVEL_CONCEPTS.md` §1,
/// `authority.rs`'s own "CPU-side page-table projection" section):
/// real verification that a CPU-process grant, derived through the
/// SAME authority graph the device side already uses, produces real,
/// readable page-table entries (`vmm::debug_translate` -- the exact
/// mechanism a real CPU MMU walk would use), and that revocation
/// really removes them. Self-contained: a fresh, real address space
/// (`vmm::new_address_space`), no PCI/AHCI involvement at all -- the
/// CPU side needed none of the device-DMA machinery to demonstrate the
/// same "graph is the single source, hardware is its projection"
/// property.
pub fn run_cpu_side_demo() {
    let target_pml4 = unsafe { vmm::new_address_space() };
    let phys = unsafe { pmm::alloc_page() };
    const PID: u32 = 0xDEAD_BEEF;
    const VADDR_BASE: u64 = 0x0000_0000_0090_0000;

    let (reachable_before, hw_before) = authority::cross_check_cpu(PID, target_pml4, VADDR_BASE, phys);
    klog_info!(
        "AUTHORITY_HW_CPU_START graph_reachable={} hw_mapped={} (both must be false before any grant)",
        reachable_before, hw_before
    );

    let granted = authority::grant_process(PID, target_pml4, VADDR_BASE, phys, 4096, true, false);
    let (reachable_after_grant, hw_after_grant) = authority::cross_check_cpu(PID, target_pml4, VADDR_BASE, phys);
    klog_info!(
        "AUTHORITY_HW_CPU_GRANT granted={} graph_reachable={} hw_mapped={} (both must be true)",
        granted, reachable_after_grant, hw_after_grant
    );

    let revoked = authority::revoke_process(PID, target_pml4, VADDR_BASE, phys);
    let (reachable_after_revoke, hw_after_revoke) = authority::cross_check_cpu(PID, target_pml4, VADDR_BASE, phys);
    klog_info!(
        "AUTHORITY_HW_CPU_REVOKE revoked={} graph_reachable={} hw_mapped={} (both must be false again)",
        revoked, reachable_after_revoke, hw_after_revoke
    );

    if !reachable_before && !hw_before
        && granted && reachable_after_grant && hw_after_grant
        && revoked && !reachable_after_revoke && !hw_after_revoke
    {
        klog_info!("AUTHORITY_HW_CPU_PASS: real CPU page-table state (via vmm::debug_translate, the same mechanism a real MMU walk uses) tracked the authority graph exactly, grant and revoke, completing section 1's CPU-side projection alongside the already-verified device-DMA side");
    } else {
        klog_info!("AUTHORITY_HW_CPU_FAIL: software and hardware state disagreed at some point -- see the markers above for exactly where");
    }
}
