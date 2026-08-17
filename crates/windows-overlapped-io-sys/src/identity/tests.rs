// Copyright (c) 2026 Mike Grier
//! Unit tests for operation identity and the live-identity registry.
//!
//! These exercise the identity logic directly against synthetic addresses; no
//! real overlapped I/O is involved, so the addresses are never dereferenced.

use std::collections::HashSet;

use windows_sys::Win32::System::IO::OVERLAPPED;

use crate::identity::{OperationId, OperationRegistry};

/// A stand-in storage address. Never dereferenced.
fn address(value: usize) -> *mut OVERLAPPED {
    value as *mut OVERLAPPED
}

// --- minting ---

#[test]
fn mint_preserves_the_address() {
    let id = OperationId::mint(address(0x1000));
    assert_eq!(id.as_ptr(), address(0x1000));
}

#[test]
fn generations_start_above_zero() {
    let id = OperationId::mint(address(0x1000));
    assert!(id.generation() > 0, "0 must never be a real generation");
}

#[test]
fn minting_the_same_address_twice_yields_distinct_identities() {
    let first = OperationId::mint(address(0x2000));
    let second = OperationId::mint(address(0x2000));
    assert_eq!(first.as_ptr(), second.as_ptr());
    assert_ne!(
        first.generation(),
        second.generation(),
        "a recycled address must not reproduce an earlier identity"
    );
    assert_ne!(first, second);
}

#[test]
fn generations_are_strictly_increasing() {
    let first = OperationId::mint(address(0x3000));
    let second = OperationId::mint(address(0x4000));
    assert!(second.generation() > first.generation());
}

#[test]
fn many_mints_are_all_distinct() {
    const MINTS: usize = 1000;
    // Deliberately reuse a small pool of addresses so only the generation can
    // distinguish the identities.
    let identities: HashSet<OperationId> = (0..MINTS)
        .map(|i| OperationId::mint(address(0x5000 + (i % 4) * 8)))
        .collect();
    assert_eq!(identities.len(), MINTS, "every mint must be unique");
}

/// Cancelling from a thread other than the submitting one is the central use of
/// an identity, so it must cross thread boundaries.
#[test]
fn identities_can_be_sent_and_shared_across_threads() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<OperationId>();
    assert_sync::<OperationId>();

    let id = OperationId::mint(address(0x6500));
    let moved = std::thread::spawn(move || (id.as_ptr() as usize, id.generation()))
        .join()
        .expect("join");
    assert_eq!(moved, (0x6500, id.generation()));
}

#[test]
fn identities_are_usable_as_hash_keys() {
    let id = OperationId::mint(address(0x6000));
    let mut set = HashSet::new();
    assert!(set.insert(id));
    assert!(!set.insert(id), "an identity must hash consistently");
}

// --- registry membership ---

#[test]
fn new_registry_is_empty() {
    let registry = OperationRegistry::new();
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());
}

#[test]
fn inserted_identity_is_live() {
    let registry = OperationRegistry::new();
    let id = OperationId::mint(address(0x7000));
    registry.insert(id);
    assert!(registry.is_live(id));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());
}

#[test]
fn removed_identity_is_no_longer_live() {
    let registry = OperationRegistry::new();
    let id = OperationId::mint(address(0x8000));
    registry.insert(id);
    assert_eq!(registry.remove(id.as_ptr()), Some(id.generation()));
    assert!(!registry.is_live(id));
    assert!(registry.is_empty());
}

#[test]
fn removing_an_unknown_address_reports_nothing() {
    let registry = OperationRegistry::new();
    assert_eq!(registry.remove(address(0x9000)), None);
}

#[test]
fn an_identity_never_inserted_is_not_live() {
    let registry = OperationRegistry::new();
    let id = OperationId::mint(address(0xA000));
    assert!(!registry.is_live(id));
}

// --- the recycling hazard this exists to stop ---

/// The core invariant: after an operation is reclaimed and its address is reused
/// by a later operation, the earlier identity must not be treated as live.
#[test]
fn a_stale_identity_does_not_match_a_recycled_address() {
    let registry = OperationRegistry::new();
    let slot = address(0xB000);

    let first = OperationId::mint(slot);
    registry.insert(first);
    registry.remove(slot);

    // The same storage is handed to a new operation.
    let second = OperationId::mint(slot);
    registry.insert(second);

    assert!(registry.is_live(second), "the live operation must match");
    assert!(
        !registry.is_live(first),
        "a retained identity must not name the operation that recycled its address"
    );
    assert_eq!(first.as_ptr(), second.as_ptr(), "the address was recycled");
}

#[test]
fn generation_of_reports_the_current_occupant() {
    let registry = OperationRegistry::new();
    let slot = address(0xC000);

    let first = OperationId::mint(slot);
    registry.insert(first);
    assert_eq!(registry.generation_of(slot), Some(first.generation()));

    registry.remove(slot);
    assert_eq!(registry.generation_of(slot), None);

    let second = OperationId::mint(slot);
    registry.insert(second);
    assert_eq!(
        registry.generation_of(slot),
        Some(second.generation()),
        "the address must report the generation of its current occupant"
    );
    assert_ne!(registry.generation_of(slot), Some(first.generation()));
}

#[test]
fn many_live_identities_are_tracked_independently() {
    const OPERATIONS: usize = 500;
    let registry = OperationRegistry::new();

    let ids: Vec<OperationId> = (0..OPERATIONS)
        .map(|i| OperationId::mint(address(0x10_000 + i * 16)))
        .collect();
    for id in &ids {
        registry.insert(*id);
    }
    assert_eq!(registry.len(), OPERATIONS);
    for id in &ids {
        assert!(registry.is_live(*id));
    }

    for id in &ids {
        registry.remove(id.as_ptr());
    }
    assert!(registry.is_empty());
    for id in &ids {
        assert!(!registry.is_live(*id));
    }
}

#[test]
fn removing_one_identity_leaves_the_others_live() {
    let registry = OperationRegistry::new();
    let first = OperationId::mint(address(0x20_000));
    let second = OperationId::mint(address(0x20_010));
    registry.insert(first);
    registry.insert(second);

    registry.remove(first.as_ptr());
    assert!(!registry.is_live(first));
    assert!(registry.is_live(second));
    assert_eq!(registry.len(), 1);
}

// --- rundown ---

#[test]
fn wait_until_empty_returns_immediately_when_empty() {
    let registry = OperationRegistry::new();
    registry.wait_until_empty();
    assert!(registry.is_empty());
}

#[test]
fn wait_until_empty_unblocks_when_the_last_operation_is_removed() {
    use std::sync::Arc;
    use std::time::Duration;

    let registry = Arc::new(OperationRegistry::new());
    let id = OperationId::mint(address(0x30_000));
    registry.insert(id);

    let remover = Arc::clone(&registry);
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        remover.remove(id.as_ptr());
    });

    registry.wait_until_empty();
    assert!(registry.is_empty());
    handle.join().expect("join the removing thread");
}

// --- backend misuse ---

/// Registering an address that is already registered is a backend defect, and
/// must fail loudly rather than corrupt the liveness answers silently.
#[test]
#[should_panic(expected = "must never be registered while it is available for reuse")]
fn inserting_the_same_address_twice_panics() {
    let registry = OperationRegistry::new();
    let slot = address(0x40_000);
    registry.insert(OperationId::mint(slot));
    // Registering a second operation at live storage is a backend bug.
    registry.insert(OperationId::mint(slot));
}

/// The panic must name the address and both generations, so a backend author
/// can tell which submission collided with which.
#[test]
fn the_duplicate_registration_panic_identifies_both_operations() {
    let registry = OperationRegistry::new();
    let slot = address(0x41_000);
    let first = OperationId::mint(slot);
    let second = OperationId::mint(slot);
    registry.insert(first);

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.insert(second);
    }))
    .expect_err("a duplicate registration must panic");

    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("the panic payload must be a message");

    assert!(
        message.contains(&format!("{slot:p}")),
        "the panic must name the colliding address; got: {message}"
    );
    assert!(
        message.contains(&first.generation().to_string()),
        "the panic must name the already-registered generation; got: {message}"
    );
    assert!(
        message.contains(&second.generation().to_string()),
        "the panic must name the incoming generation; got: {message}"
    );
    assert!(
        message.contains("defect in the completion backend"),
        "the panic must say whose bug this is; got: {message}"
    );
}
