//! Phase 12 exit criterion 4 (`docs/ROADMAP.md` §5), minimal real
//! slice: "Human keyboard/mouse input reaches the correct focused
//! window with no cross-window leakage." Real, disclosed scope: PS/2
//! keyboard only (Phase 11's USB HID path remains blocked on its own
//! open Configure Endpoint bug, unrelated to this), raw scancodes only
//! (no decode/ASCII layer), and a single, kernel-assigned initial focus
//! — there is no window manager or click-to-focus yet, so which window
//! starts focused is a fixed, disclosed convention
//! (`compositor.rs::spawn_window_client` assigns it to whichever client
//! is labeled the primary one), not something either window process
//! ever decides for itself. What IS real: a genuine per-window IPC
//! endpoint, a real routing decision keyed on which window currently
//! holds focus, and real delivery — provably NEVER to the other window
//! — via the exact same `ipc.rs` rendezvous mechanism every other IPC
//! path in this kernel already uses.

use crate::capability::{CapId, CapabilityTable, ObjectId, Rights};
use crate::{ipc, klog_info, thread};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

struct WindowInput {
    surface_object: ObjectId,
    send_cap: CapId,
}

static mut REGISTRY: Option<Vec<WindowInput>> = None;
// Kernel-side table holding the SEND-rights half of every registered
// window's input endpoint -- mirrors `FB_READY_TABLE`'s own pattern
// (`syscall.rs`) of a dedicated table for a specific kernel-mediated
// operation, just per-window instead of global.
static mut SENDER_TABLE: Option<CapabilityTable> = None;
const NO_FOCUS: u32 = u32::MAX;
static FOCUSED_SURFACE: AtomicU32 = AtomicU32::new(NO_FOCUS);

#[allow(static_mut_refs)]
fn registry_mut() -> &'static mut Vec<WindowInput> {
    unsafe {
        let slot = &mut *&raw mut REGISTRY;
        if slot.is_none() {
            *slot = Some(Vec::new());
        }
        slot.as_mut().unwrap()
    }
}

#[allow(static_mut_refs)]
fn sender_table_mut() -> &'static mut CapabilityTable {
    unsafe {
        let slot = &mut *&raw mut SENDER_TABLE;
        if slot.is_none() {
            *slot = Some(CapabilityTable::new());
        }
        slot.as_mut().unwrap()
    }
}

/// Real, one-time setup for a window that wants routed input: mints a
/// fresh `IpcEndpoint` object, grants its RECEIVE half into the CALLING
/// thread's own `cap_table` (`thread::grant_current_capability` — the
/// same real fix `compositor.rs`'s own doc describes for why a
/// throwaway local table would never reach the process about to enter
/// ring 3), keeps the SEND half in this module's own internal table for
/// `deliver_key_event` to use later, and records the (surface,
/// endpoint) pair. Returns the RECEIVE-side `CapId` the calling window
/// process should remember and pass to its own ring-3 code, the same
/// way it already tracks its `Surface` capability's `CapId`.
pub fn register_window_input(surface_object: ObjectId) -> CapId {
    let send_cap = ipc::create_endpoint(sender_table_mut(), Rights::SEND);
    let endpoint_object = sender_table_mut().resolve(send_cap, Rights::SEND).unwrap().object_id;
    let receive_cap = thread::grant_current_capability(endpoint_object, Rights::RECEIVE);
    registry_mut().push(WindowInput { surface_object, send_cap });
    klog_info!("INPUT_WINDOW_REGISTERED surface={} endpoint={}", surface_object, endpoint_object);
    receive_cap
}

/// Real, kernel-side (never process-self-declared) focus assignment —
/// see this module's own doc for why the window itself never gets to
/// call this on its own behalf.
pub fn set_focus(surface_object: ObjectId) {
    FOCUSED_SURFACE.store(surface_object, Ordering::SeqCst);
    klog_info!("INPUT_FOCUS_SET surface={}", surface_object);
}

/// Real routing decision: deliver `scancode` to whichever window
/// currently holds focus. A key event while a DIFFERENT (or no) window
/// has focus is a real, disclosed no-op — dropped, never queued and
/// never delivered to the wrong window, which is the actual isolation
/// property this exit criterion cares about (no cross-window leakage
/// is stronger, and easier to get right, than "eventually delivered to
/// someone").
pub fn deliver_key_event(scancode: u8) {
    crate::compositor_metrics::record_keyboard_event();
    crate::input_queue::enqueue(crate::input_queue::InputEventKind::Key { scancode });

    // Hotkeys for automated benchmarking & telemetry
    match scancode {
        0x3B => {
            // F1: Start baseline benchmark, reset metrics
            crate::compositor_metrics::reset_metrics();
            klog_info!("[BENCHMARK_SCENARIO_START] scenario=idle");
            return;
        }
        0x3C => {
            // F2: End idle, dump summary
            crate::compositor_metrics::dump_summary("idle");
            klog_info!("[BENCHMARK_SCENARIO_START] scenario=mouse_motion");
            return;
        }
        0x3D => {
            // F3: End mouse_motion, dump summary
            crate::compositor_metrics::dump_summary("mouse_motion");
            klog_info!("[BENCHMARK_SCENARIO_START] scenario=window_drag");
            return;
        }
        0x3E => {
            // F4: End window_drag, dump summary
            crate::compositor_metrics::dump_summary("window_drag");
            klog_info!("[BENCHMARK_SCENARIO_START] scenario=typing");
            return;
        }
        0x3F => {
            // F5: End typing, dump summary
            crate::compositor_metrics::dump_summary("typing");
            klog_info!("[BENCHMARK_SCENARIO_START] scenario=multi_window");
            return;
        }
        0x40 => {
            // F6: End multi_window, dump summary
            crate::compositor_metrics::dump_summary("multi_window");
            klog_info!("[BENCHMARK_SCENARIO_START] scenario=rapid_mouse");
            return;
        }
        0x41 => {
            // F7: End rapid_mouse, dump summary
            crate::compositor_metrics::dump_summary("rapid_mouse");
            klog_info!("[BENCHMARK_SCENARIO_END]");
            return;
        }
        0x58 => {
            // F12: Snapshot dump
            crate::compositor_metrics::dump_summary("snapshot");
            return;
        }
        // Consume break codes for F1..F7 and F12
        0xBB..=0xC1 | 0xD8 => {
            return;
        }
        _ => {}
    }

    let focused = FOCUSED_SURFACE.load(Ordering::SeqCst);
    if focused == NO_FOCUS {
        klog_info!("INPUT_ROUTE_DROPPED_NO_FOCUS scancode=0x{:x}", scancode);
        return;
    }
    let Some(w) = registry_mut().iter().find(|w| w.surface_object == focused) else {
        klog_info!("INPUT_ROUTE_DROPPED_UNKNOWN_FOCUS surface={} scancode=0x{:x}", focused, scancode);
        return;
    };
    let mut msg = ipc::Message::default();
    msg.data[0] = scancode as u64;
    // Fire-and-forget (`ipc::try_send`, not the blocking `ipc::send`) --
    // see that function's own doc for the real deadlock this fixes: a
    // window's own input poll is bounded, so waiting here for
    // confirmed consumption could spin forever if that poll window had
    // already closed.
    match ipc::try_send(sender_table_mut(), w.send_cap, msg) {
        Ok(true) => klog_info!("INPUT_ROUTE_OK surface={} scancode=0x{:x}", focused, scancode),
        Ok(false) => klog_info!("INPUT_ROUTE_DROPPED_BUSY surface={} scancode=0x{:x}", focused, scancode),
        Err(e) => klog_info!("INPUT_ROUTE_FAILED surface={} {:?}", focused, e),
    }
}
