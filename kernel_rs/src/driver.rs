//! Capability-gated hardware access. Phase 3's "driver process model:
//! MMIO regions and interrupt lines granted as capabilities, nothing
//! ambient" (`docs/ROADMAP.md`). Every function here resolves a real
//! capability before touching hardware — there is no other path in this
//! kernel (after this module exists) for user-space code to reach MMIO,
//! an interrupt line, or an I/O port.

use crate::capability::{CapError, CapId, CapabilityTable, KernelObjectKind, Rights};
use crate::{interrupt_forward, vmm};

/// Bump allocator for the user-visible MMIO window — deliberately separate
/// from `vmm::MMIO_VIRTUAL_BASE` (the KERNEL's own MMIO window, e.g. where
/// the LAPIC is mapped): a driver process must never be able to guess or
/// reach the kernel's own MMIO mappings, only what it's explicitly handed
/// back from `map_mmio`.
pub const USER_MMIO_WINDOW_BASE: u64 = 0x0000_0000_2000_0000;
static NEXT_USER_MMIO_OFFSET: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Creates a new MMIO-region kernel object and grants a capability to it
/// in `table`. This is the ONLY way an `MmioRegion` capability comes into
/// existence — called by whatever sets up a driver process (the device
/// manager, Phase 3's later item), never by the driver itself (a driver
/// can't grant itself access to arbitrary physical memory; it can only
/// receive a capability someone else already decided to hand it).
pub fn create_mmio_capability(table: &mut CapabilityTable, phys_base: u64, size: u64, rights: Rights) -> CapId {
    let object_id = crate::capability::create_object(KernelObjectKind::MmioRegion { phys_base, size });
    table.grant(object_id, rights)
}

pub fn create_interrupt_capability(table: &mut CapabilityTable, vector: u8, rights: Rights) -> CapId {
    let object_id = crate::capability::create_object(KernelObjectKind::InterruptLine { vector });
    interrupt_forward::register(vector);
    table.grant(object_id, rights)
}

pub fn create_port_capability(table: &mut CapabilityTable, base: u16, count: u16, rights: Rights) -> CapId {
    let object_id = crate::capability::create_object(KernelObjectKind::PortIoRange { base, count });
    table.grant(object_id, rights)
}

/// Phase 10 deliverable 1: mints a real `Socket` capability object and
/// grants it into `table` — same real pattern as every other
/// `create_*_capability` here (create the typed object, grant it,
/// return the `CapId`). Kernel-mode setup code only, matching this
/// entire module's own discipline: no running ring-3 process can mint
/// its own capabilities, only receive ones granted at spawn time.
///
/// `rights` deliberately reuses `Rights::SEND`/`Rights::RECEIVE` rather
/// than minting dedicated `SOCKET_*` bits — a socket IS, semantically,
/// an IPC channel to the network (may send data out, may receive data
/// in), the same real actions those two bits already name for
/// `IpcEndpoint`. This differs from `MAP`/`WAIT`/`PORT_IO`, which each
/// got their own bit because THEIR actions have no existing analogue.
pub fn create_socket_capability(
    table: &mut CapabilityTable,
    protocol: crate::capability::SocketProtocol,
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    rights: Rights,
) -> CapId {
    let object_id = crate::capability::create_object(KernelObjectKind::Socket {
        protocol,
        local_port,
        remote_ip,
        remote_port,
    });
    table.grant(object_id, rights)
}

/// Phase 12 deliverable 1: mints a real `Surface` capability object
/// and grants it into `table` — same real pattern as every other
/// `create_*_capability` here. Only `compositor.rs` calls this (the
/// kernel decides what real screen region exists; a window process
/// only ever receives the bounds it was granted, never picks its
/// own).
pub fn create_surface_capability(table: &mut CapabilityTable, x: u32, y: u32, width: u32, height: u32, rights: Rights) -> CapId {
    let object_id = crate::capability::create_object(KernelObjectKind::Surface { x, y, width, height });
    table.grant(object_id, rights)
}

#[derive(Debug)]
pub enum DriverError {
    Cap(CapError),
    WrongObjectKind,
}

/// Maps an `MmioRegion` capability's physical range into the CURRENT
/// thread's address space (via `thread::current_kernel_stack_top`'s same
/// "operate on whichever thread is actually running" pattern — see
/// `thread::current_address_space`), page by page, RW+NX+cache-disabled
/// (matching `vmm::map_mmio_page`'s reasoning: MMIO must never be cached).
/// Returns the virtual address the region now starts at.
pub fn map_mmio(table: &CapabilityTable, cap_id: CapId, into_pml4: u64) -> Result<u64, DriverError> {
    let cap = table.resolve(cap_id, Rights::MAP).map_err(DriverError::Cap)?;
    let (phys_base, size) = match crate::capability::object_kind(cap.object_id) {
        Some(KernelObjectKind::MmioRegion { phys_base, size }) => (phys_base, size),
        _ => return Err(DriverError::WrongObjectKind),
    };

    let pages = (size + 4095) / 4096;
    let offset = NEXT_USER_MMIO_OFFSET.fetch_add(
        pages * 4096,
        core::sync::atomic::Ordering::SeqCst,
    );
    let vaddr_base = USER_MMIO_WINDOW_BASE + offset;

    for p in 0..pages {
        unsafe {
            vmm::map_page_in(
                into_pml4,
                vaddr_base + p * 4096,
                phys_base + p * 4096,
                vmm::PAGE_USER
                    | vmm::PAGE_WRITABLE
                    | vmm::PAGE_NO_EXECUTE
                    | vmm::PAGE_CACHE_DISABLE,
            );
        }
    }
    Ok(vaddr_base)
}

/// Resolves an `InterruptLine` capability, then blocks until that vector
/// fires — capability-gated version of `interrupt_forward::wait_for_interrupt`.
pub fn wait_interrupt(table: &CapabilityTable, cap_id: CapId) -> Result<(), DriverError> {
    let cap = table.resolve(cap_id, Rights::WAIT).map_err(DriverError::Cap)?;
    match crate::capability::object_kind(cap.object_id) {
        Some(KernelObjectKind::InterruptLine { vector }) => {
            interrupt_forward::wait_for_interrupt(vector);
            Ok(())
        }
        _ => Err(DriverError::WrongObjectKind),
    }
}

pub fn ack_interrupt(table: &CapabilityTable, cap_id: CapId) -> Result<(), DriverError> {
    let cap = table.resolve(cap_id, Rights::WAIT).map_err(DriverError::Cap)?;
    match crate::capability::object_kind(cap.object_id) {
        Some(KernelObjectKind::InterruptLine { vector }) => {
            interrupt_forward::acknowledge(vector);
            Ok(())
        }
        _ => Err(DriverError::WrongObjectKind),
    }
}

/// Resolves a `PortIoRange` capability and opens EXACTLY those ports in
/// the CALLING THREAD's OWN TSS IOPB copy — not full IOPL=3 (which would
/// open every port to any ring-3 code regardless of what it actually
/// holds a capability for), and — real bug found and fixed via Phase
/// 7's shell (see `gdt.rs`'s `set_iopb` doc comment for the full
/// story) — not the single global TSS either, which used to leak every
/// granted port to every OTHER ring-3 thread permanently.
pub fn grant_port_access(table: &CapabilityTable, cap_id: CapId) -> Result<(), DriverError> {
    let cap = table.resolve(cap_id, Rights::PORT_IO).map_err(DriverError::Cap)?;
    match crate::capability::object_kind(cap.object_id) {
        Some(KernelObjectKind::PortIoRange { base, count }) => {
            for port in base..base.saturating_add(count) {
                crate::thread::allow_port_for_current(port);
            }
            Ok(())
        }
        _ => Err(DriverError::WrongObjectKind),
    }
}
