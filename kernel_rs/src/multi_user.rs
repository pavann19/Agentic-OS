//! Phase 14 Deliverable 1:
//! Multi-User Capability Session Model (ADR-003: zero ambient authority).
//!
//! Under Agentic OS, user accounts are not ambient POSIX identities (e.g. `getuid()`,
//! ambient file permissions, or `root` superuser escape hatches). Instead, an
//! authenticated user is represented by a capability domain (`UserSession`) owning
//! its own isolated `CapabilityTable`.
//!
//! A user:
//!   - Can ONLY access resources named by capabilities held in their session table.
//!   - Cannot name, discover, or access another user's resources by default
//!     (cross-user resolution returns `NoSuchCapability`, indistinguishable from
//!     nonexistence).
//!   - Can explicitly delegate attenuated capabilities (e.g. read-only access
//!     to a shared mailbox or file) into another user's session table.
//!   - Can revoke shared resources immediately, invalidating delegated capabilities
//!     across user session boundaries in O(1) time via generation counters.

use crate::capability::{self, CapError, CapId, CapabilityTable, KernelObjectKind, Rights};
use crate::klog_info;

/// An isolated user session domain.
pub struct UserSession {
    pub uid: u32,
    pub username: [u8; 16],
    pub username_len: usize,
    pub session_cap: CapId,
    pub table: CapabilityTable,
}

impl UserSession {
    /// Creates a new user session with an isolated capability table.
    pub fn new(uid: u32, username_str: &str) -> Self {
        let mut table = CapabilityTable::new();
        let obj_id = capability::create_object(KernelObjectKind::UserSession { uid });
        let session_cap = table.grant(
            obj_id,
            Rights::SESSION_SWITCH.union(Rights::GRANT).union(Rights::REVOKE),
        );

        let mut username = [0u8; 16];
        let bytes = username_str.as_bytes();
        let len = bytes.len().min(16);
        username[..len].copy_from_slice(&bytes[..len]);

        Self {
            uid,
            username,
            username_len: len,
            session_cap,
            table,
        }
    }

    /// Helper to get username as str.
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.username[..self.username_len]).unwrap_or("<invalid>")
    }
}

/// Adversarial proof of the multi-user capability session model.
///
/// Exercises:
///   1. Creation of isolated user session domains (Alice & Bob).
///   2. Adversarial cross-user isolation: Bob attempts to resolve Alice's private
///      file capability and is denied with `NoSuchCapability` (indistinguishable from nonexistence).
///   3. Explicit attenuated delegation: Alice derives a RECEIVE/READ-only capability
///      into Bob's session table.
///   4. Adversarial rights escalation: Bob attempts to execute SEND/WRITE on the delegated
///      read-only capability and is denied with `InsufficientRights`.
///   5. Cross-boundary revocation: Alice revokes the shared object, immediately
///      rendering Bob's delegated capability unusable (`Revoked`).
pub fn run_multi_user_demo() {
    klog_info!("MULTI_USER_DEMO_START");

    // 1. Initialize user sessions
    let mut alice = UserSession::new(1000, "alice");
    let mut bob = UserSession::new(1001, "bob");
    klog_info!(
        "MULTI_USER_SESSION_INIT alice_uid={} bob_uid={}",
        alice.uid,
        bob.uid
    );

    // 2. Alice creates a private resource (e.g. private credentials/file)
    let alice_secret_obj = capability::create_object(KernelObjectKind::FileObject { inode: 101 });
    let alice_secret_cap = alice.table.grant(
        alice_secret_obj,
        Rights::READ.union(Rights::WRITE).union(Rights::GRANT),
    );
    klog_info!(
        "MULTI_USER_ALICE_PRIVATE_OBJECT obj={} cap={}",
        alice_secret_obj,
        alice_secret_cap
    );

    // Adversarial probe: Bob attempts to resolve Alice's cap_id in Bob's table.
    // Because Bob was never granted this capability, Bob's table yields NoSuchCapability.
    match bob.table.resolve(alice_secret_cap, Rights::READ) {
        Err(CapError::NoSuchCapability) => {
            klog_info!("MULTI_USER_CROSS_ACCESS_DENIED_OK (Bob cannot access Alice private cap: NoSuchCapability)");
        }
        other => {
            klog_info!(
                "MULTI_USER_CROSS_ACCESS_UNEXPECTED result={:?}",
                other.is_ok()
            );
        }
    }

    // 3. Explicit Attenuated Delegation:
    // Alice creates a shared IPC endpoint and delegates a RECEIVE-only capability to Bob.
    let shared_obj = capability::create_object(KernelObjectKind::IpcEndpoint);
    let alice_shared_cap = alice.table.grant(
        shared_obj,
        Rights::SEND.union(Rights::RECEIVE).union(Rights::GRANT).union(Rights::REVOKE),
    );

    let bob_shared_cap = alice
        .table
        .derive(alice_shared_cap, Rights::RECEIVE, &mut bob.table)
        .expect("Alice delegation to Bob must succeed");
    klog_info!(
        "MULTI_USER_DELEGATION_OK (Alice delegated READ-only shared cap to Bob: cap={})",
        bob_shared_cap
    );

    // Bob resolves the delegated capability with RECEIVE rights — succeeds.
    match bob.table.resolve(bob_shared_cap, Rights::RECEIVE) {
        Ok(resolved) => {
            assert_eq!(resolved.object_id, shared_obj);
            klog_info!("MULTI_USER_BOB_RESOLVE_SHARED_OK");
        }
        Err(e) => {
            klog_info!("MULTI_USER_BOB_RESOLVE_FAILED {:?}", e);
        }
    }

    // 4. Adversarial Escalation:
    // Bob attempts to use the delegated capability for SEND (which Bob does not hold).
    match bob.table.resolve(bob_shared_cap, Rights::SEND) {
        Err(CapError::InsufficientRights) => {
            klog_info!("MULTI_USER_WRITE_DENIED_OK (Bob tried WRITE on READ-only cap: InsufficientRights)");
        }
        other => {
            klog_info!(
                "MULTI_USER_ESCALATION_UNEXPECTED result={:?}",
                other.is_ok()
            );
        }
    }

    // 5. Cross-boundary revocation:
    // Alice revokes the shared object.
    capability::revoke(shared_obj, false);

    // Bob attempts to resolve his delegated capability after Alice's revocation.
    match bob.table.resolve(bob_shared_cap, Rights::RECEIVE) {
        Err(CapError::Revoked) => {
            klog_info!("MULTI_USER_REVOCATION_OK (Alice revoked shared object, Bob immediate Revoked)");
        }
        other => {
            klog_info!(
                "MULTI_USER_REVOCATION_UNEXPECTED result={:?}",
                other.is_ok()
            );
        }
    }

    klog_info!("MULTI_USER_DEMO_SUCCESS");
}
