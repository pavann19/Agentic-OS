//! Phase 13 deliverable 1's own real, adversarial proof — closing this
//! increment's exit criterion ("Installing an app grants exactly the
//! capabilities its manifest declared, nothing ambient — an app that
//! tries to use an undeclared capability is denied, demonstrated
//! adversarially"). `manifest.rs`/`thread::spawn_with_manifest` are the
//! real mechanism; this module is what actually exercises it, the same
//! "prove it, don't just build it" discipline `socket_demo.rs` already
//! established for Phase 10's revocation exit criterion.
//!
//! Real, disclosed scope: this proves the MANIFEST enforcement
//! mechanism (declare, grant, deny) completely and adversarially, using
//! two already-real `KernelObjectKind` variants (`Surface`, `Socket`) as
//! its two test capabilities. It does not yet involve an actual
//! installer service reading a manifest off disk (deliverable 2, real
//! separate follow-up work) — the `Manifest` value here is built
//! in-kernel, the same honest stand-in `socket_demo.rs` itself used for
//! "a real Socket capability" before any live TCP connection backed one.
#![cfg(feature = "manifest_demo")]

use crate::capability::{self, KernelObjectKind, Rights, SocketProtocol};
use crate::manifest::{CapKind, Manifest};
use crate::{klog_info, thread, vmm};

/// The app's own declared manifest: "I need a Surface. Nothing else." —
/// built here as the honest stand-in for a real installer parsing this
/// off disk (deliverable 2, not yet built).
fn app_manifest() -> Manifest {
    Manifest::NONE.allow(CapKind::Surface)
}

pub fn start() {
    let surface_object = capability::create_object(KernelObjectKind::Surface { x: 0, y: 0, width: 64, height: 64 });
    let socket_object = capability::create_object(KernelObjectKind::Socket {
        protocol: SocketProtocol::Tcp,
        local_port: 51001,
        remote_ip: [93, 184, 216, 34],
        remote_port: 80,
    });
    klog_info!("MANIFEST_DEMO_START surface={} socket={}", surface_object, socket_object);

    // Real, deliberate over-ask: the installer/spawner attempts to grant
    // BOTH a Surface (declared) and a Socket (never declared) — exactly
    // the adversarial shape the exit criterion names: "an app that
    // TRIES to use an undeclared capability is denied."
    thread::spawn_with_manifest(
        app_thread,
        vmm::kernel_pml4_phys(),
        &[
            (surface_object, Rights::MAP),
            (socket_object, Rights::SEND.union(Rights::RECEIVE)),
        ],
        app_manifest(),
    );
}

const SURFACE_CAP: capability::CapId = 0;
// The Socket grant above was refused before it ever reached the table,
// so no second slot was ever pushed (`CapabilityTable::grant` assigns
// CapId by table length AT GRANT TIME, not by request-list position) --
// slot 1 genuinely does not exist. Probing it is the real adversarial
// check itself, not a contrived off-by-one.
const NEVER_GRANTED_SOCKET_CAP: capability::CapId = 1;

extern "C" fn app_thread() {
    match thread::resolve_current_capability(SURFACE_CAP, Rights::MAP) {
        Ok(_) => klog_info!("MANIFEST_DEMO_DECLARED_GRANT_OK cap={} (Surface, as manifested)", SURFACE_CAP),
        Err(e) => klog_info!("MANIFEST_DEMO_DECLARED_GRANT_UNEXPECTED_FAIL {:?}", e),
    }

    match thread::resolve_current_capability(NEVER_GRANTED_SOCKET_CAP, Rights::SEND) {
        Ok(_) => klog_info!("MANIFEST_DEMO_UNDECLARED_GRANT_LEAKED cap={} -- BUG: Socket resolved despite no manifest declaration", NEVER_GRANTED_SOCKET_CAP),
        Err(e) => klog_info!("MANIFEST_DEMO_UNDECLARED_GRANT_DENIED_OK cap={} correctly refuses to resolve: {:?}", NEVER_GRANTED_SOCKET_CAP, e),
    }
}
