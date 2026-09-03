//! Phase 5 — tool/intent surface (`docs/ROADMAP.md` §5 Phase 5,
//! deliverable 3): "capability invocations exposed as typed,
//! discoverable operations with declared preconditions and effects."
//!
//! Scope, stated honestly: this is a real, typed catalog an agent
//! process can query BEFORE knowing anything about this kernel's
//! syscall numbers — not a general dynamic plugin registry, and not
//! (yet) how invocation actually happens (an agent still calls the raw
//! syscall number directly once it knows it — a genuine typed
//! dispatch-by-tool-id invocation path is real future work). What's
//! real here: an agent no longer has to have this kernel's syscall
//! surface hardcoded into its own binary to know what it CAN ask for
//! and what holding it would require — it can discover that from a
//! real, typed catalog the kernel itself serves, unconditionally (see
//! syscall 8 in `syscall.rs` — discovery itself needs no capability,
//! matching "discoverable"; using what's discovered still goes through
//! every existing capability check).

/// One entry in the catalog. `syscall_num` is this tool's real
/// invocation path (today: a raw syscall number — see the scope note
/// above); `required_rights` is the exact `Rights` bitmask
/// `CapabilityTable::resolve` will check before allowing it;
/// `side_effecting` is 0 for a tool that only reads kernel state
/// (nothing changes as a result of calling it) or 1 for one that does.
/// This is the "declared preconditions and effects" Phase 5 asks for,
/// kept to the two properties this kernel can currently state
/// truthfully about every tool it exposes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ToolDescriptor {
    pub syscall_num: u32,
    pub required_rights: u32,
    pub side_effecting: u32,
    _pad: u32,
}

pub const TOOL_DESCRIPTOR_SIZE: u64 = 16; // 4+4+4+4, naturally 8-byte-friendly at this width

/// The real catalog — every capability-invoking operation this kernel
/// currently exposes to an agent process. Extending Phase 5 with a new
/// agent-facing syscall means adding one entry here, not just wiring
/// the dispatch arm — the whole point is that this list and what's
/// actually callable never drift apart.
pub fn catalog() -> [ToolDescriptor; 2] {
    [
        ToolDescriptor {
            syscall_num: 7,
            required_rights: crate::capability::Rights::INTROSPECT.0,
            side_effecting: 0,
            _pad: 0,
        },
        ToolDescriptor {
            syscall_num: 9,
            required_rights: crate::capability::Rights::AUDIT_QUERY.0,
            side_effecting: 0,
            _pad: 0,
        },
    ]
}

pub fn tool_descriptor_bytes(d: &ToolDescriptor) -> [u8; TOOL_DESCRIPTOR_SIZE as usize] {
    let syscall_num = d.syscall_num.to_le_bytes();
    let required_rights = d.required_rights.to_le_bytes();
    let side_effecting = d.side_effecting.to_le_bytes();
    let mut out = [0u8; TOOL_DESCRIPTOR_SIZE as usize];
    let mut i = 0;
    while i < 4 {
        out[i] = syscall_num[i];
        i += 1;
    }
    while i < 8 {
        out[i] = required_rights[i - 4];
        i += 1;
    }
    while i < 12 {
        out[i] = side_effecting[i - 8];
        i += 1;
    }
    // bytes 12..16 stay zero (`_pad`)
    out
}
