// Copyright (c) 2026 Mike Grier
//! Integration tests for operation-identity durability on the raw IOCP backend.
//!
//! An identity must name the operation it was issued for and no other. Because
//! an operation's storage address is returned to the allocator when it is
//! reclaimed, a later operation can be handed that same address; without the
//! generation stamped at submission, an identity retained past its operation's
//! completion would name whichever operation now occupies the address, and
//! `cancel` -- a safe function -- would act on it.

#![cfg(windows)]

use std::collections::HashSet;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_overlapped_io_sys::{
    CompletionPort, Issued, Operation, OperationId, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::OVERLAPPED;

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-identity-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

fn open_overlapped(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open overlapped endpoint")
}

/// Assert that the registry refused an identity *before* any native call.
///
/// This distinguishes the guarantee under test from `CancelIoEx` happening to
/// report `ERROR_NOT_FOUND`: a registry rejection is constructed in Rust and so
/// carries no OS error code, whereas a kernel rejection always does. Only the
/// former proves that a recycled address was never handed to the kernel.
fn assert_rejected_without_a_native_call(error: &io::Error) {
    assert_eq!(
        error.kind(),
        io::ErrorKind::NotFound,
        "a stale identity must be reported as NotFound"
    );
    assert!(
        error.raw_os_error().is_none(),
        "the identity must be rejected by the registry before CancelIoEx runs, but this error \
         came from the kernel: {error:?}"
    );
}

/// A connected server pipe end carrying no data, so a read on it stays pending
/// until it is cancelled. The client end must be kept alive by the caller.
fn pending_pipe(tag: &str) -> (UnassociatedEndpoint, std::fs::File) {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
    use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

    /// `PIPE_ACCESS_DUPLEX`. Changing this value is a breaking change.
    const PIPE_ACCESS_DUPLEX: u32 = 0x0000_0003;
    /// `PIPE_TYPE_BYTE`. Changing this value is a breaking change.
    const PIPE_TYPE_BYTE: u32 = 0x0000_0000;

    // Built from an escaped separator rather than a literal UNC prefix, so the
    // name survives any tooling that mangles adjacent backslashes.
    let sep = '\u{5c}';
    let name = format!(
        "{sep}{sep}.{sep}pipe{sep}windows-overlapped-io-sys-identity-{tag}-{}",
        std::process::id()
    );
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: creates a fresh overlapped named pipe from a valid wide name.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE,
            1,
            4096,
            4096,
            0,
            ptr::null(),
        )
    };
    assert!(
        !handle.is_null() && handle as isize != -1,
        "CreateNamedPipeW failed: {}",
        io::Error::last_os_error()
    );

    // SAFETY: fresh, exclusively owned, and opened with FILE_FLAG_OVERLAPPED.
    let endpoint =
        unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(handle)) };
    let client = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect the client pipe end");
    (endpoint, client)
}

/// Issue a one-byte overlapped read into `slot`.
///
/// SAFETY: `slot` must point to at least one writable byte that stays valid
/// until the operation completes.
unsafe fn issue_read(
    handle: std::os::windows::io::BorrowedHandle<'_>,
    overlapped: *mut OVERLAPPED,
    slot: *mut u8,
) -> io::Result<Issued> {
    // SAFETY: forwarded from this function's own contract.
    let ok = unsafe { ReadFile(handle.as_raw_handle(), slot, 1, ptr::null_mut(), overlapped) };
    if ok != 0 {
        return Ok(Issued::Pending);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
        return Ok(Issued::Pending);
    }
    Err(error)
}

/// Submit, complete, and reclaim one operation, returning its now-stale identity.
fn spend_one_operation(
    endpoint: &windows_overlapped_io_sys::AssociatedEndpoint<'_>,
    port: &CompletionPort,
    slot: *mut u8,
    payload: usize,
) -> OperationId {
    let operation = Operation::new(payload);
    // SAFETY: one 1-byte overlapped ReadFile into `slot`, which the caller keeps
    // alive; the completion is drained before this function returns.
    let submitted =
        unsafe { endpoint.submit(operation, |handle, ov| issue_read(handle, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected a pending submission, got {other:?}"),
    };
    let completion = port.get(5_000).expect("get").expect("a completion");
    // SAFETY: matches the Operation<usize> submitted just above, claimed once.
    let _operation = unsafe { completion.claim::<usize>() };
    assert_eq!(port.outstanding(), 0, "the operation must be reclaimed");
    id
}

// --- identities are distinct even when addresses repeat ---

/// Repeatedly spending one operation reuses storage addresses, and the identity
/// must still differ every time.
#[test]
fn recycled_addresses_still_produce_distinct_identities() {
    const CYCLES: usize = 64;

    let path = temp_file_with(b"identity durability", "recycle");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x50)
        .expect("associate");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let mut identities = HashSet::new();
    let mut addresses = HashSet::new();
    for cycle in 0..CYCLES {
        let id = spend_one_operation(&endpoint, &port, slot, cycle);
        assert!(
            identities.insert(id),
            "cycle {cycle} reproduced an earlier identity"
        );
        addresses.insert(id.as_ptr() as usize);
    }

    assert_eq!(identities.len(), CYCLES, "every identity must be unique");

    // Whether the allocator actually reuses an address in a given run depends on
    // it and on whatever else the process is doing, so reuse is reported rather
    // than required -- asserting it made this test flaky. The reuse case is
    // covered deterministically by `a_stale_generation_at_a_live_address_is_rejected`
    // below, which builds the exact identity a reused address would produce.
    if addresses.len() == CYCLES {
        eprintln!(
            "note: no storage address was reused across {CYCLES} cycles in this run, so this \
             test exercised only identity uniqueness"
        );
    }
    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

// --- the hazard: a retained identity must not cancel a later operation ---

/// A retained identity whose operation has completed must be rejected, even once
/// its address has been handed to a live operation.
#[test]
fn a_retained_identity_cannot_cancel_the_operation_that_recycled_its_address() {
    const CYCLES: usize = 64;

    let path = temp_file_with(b"identity durability", "stale-cancel");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x51)
        .expect("associate");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    // Spend operations until one of their addresses is reused by a live
    // operation, which is the situation the generation exists to disambiguate.
    let mut stale: Vec<OperationId> = Vec::new();
    let mut collided = None;
    for cycle in 0..CYCLES {
        let dead = spend_one_operation(&endpoint, &port, slot, cycle);
        stale.push(dead);

        let operation = Operation::new(1000 + cycle);
        // SAFETY: one 1-byte overlapped ReadFile into `landed`, which outlives
        // the operation because it is drained below before the test returns.
        let submitted =
            unsafe { endpoint.submit(operation, |handle, ov| issue_read(handle, ov, slot)) };
        let live = match submitted {
            Submitted::Pending(id) => id,
            other => panic!("expected a pending submission, got {other:?}"),
        };

        let reused = stale
            .iter()
            .find(|dead| dead.as_ptr() == live.as_ptr())
            .copied();

        if let Some(dead) = reused {
            // The live operation occupies a dead operation's address. The stale
            // identity must be rejected rather than cancelling this operation.
            assert_ne!(
                dead.generation(),
                live.generation(),
                "the recycled address must carry a new generation"
            );

            let rejected = endpoint
                .cancel(dead)
                .expect_err("a stale identity must not be accepted for cancellation");
            assert_rejected_without_a_native_call(&rejected);

            // The live operation must be entirely undisturbed by that attempt.
            assert_eq!(port.outstanding(), 1, "the live operation must survive");
            collided = Some((dead, live));
        }

        // Drain the live operation for this cycle. That it completes normally --
        // rather than as ERROR_OPERATION_ABORTED -- is the proof that the stale
        // cancel above did not reach it.
        let completion = port.get(5_000).expect("get").expect("a completion");
        if collided.is_some() {
            assert!(
                completion.error().is_none(),
                "the live operation was disturbed by a stale cancel: {:?}",
                completion.error()
            );
        }
        // SAFETY: matches the Operation<usize> submitted above, claimed once.
        let _operation = unsafe { completion.claim::<usize>() };
        assert_eq!(port.outstanding(), 0);
        stale.push(live);

        if collided.is_some() {
            break;
        }
    }

    // Natural address reuse is opportunistic, so its absence is reported rather
    // than failed -- requiring it here made this test flaky. The hazard itself is
    // covered on every run by `a_stale_generation_at_a_live_address_is_rejected`.
    match collided {
        Some((dead, live)) => assert_eq!(
            dead.as_ptr(),
            live.as_ptr(),
            "the collision must be a genuine address reuse"
        ),
        None => eprintln!(
            "note: no storage address was reused across {CYCLES} cycles in this run, so this \
             test observed no natural collision"
        ),
    }
    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// The recycled-address hazard, synthesized deterministically.
///
/// An identity carrying an *older* generation at an address that is currently
/// live is exactly the value a retained identity would have after the allocator
/// reissued its storage. Building it directly with `OperationId::from_parts`
/// removes the dependence on the allocator actually reusing an address, so this
/// covers the hazard on every run rather than opportunistically.
#[test]
fn a_stale_generation_at_a_live_address_is_rejected() {
    let (pipe, _client) = pending_pipe("synthetic-aba");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port.associate(pipe, 0x58).expect("associate");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(11_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted =
        unsafe { endpoint.submit(operation, |handle, ov| issue_read(handle, ov, slot)) };
    let live = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };
    assert_eq!(port.outstanding(), 1);

    // The identity a previous operation at this same storage would have had.
    let stale = OperationId::from_parts(live.as_ptr(), live.generation() - 1);
    assert_eq!(
        stale.as_ptr(),
        live.as_ptr(),
        "same address by construction"
    );
    assert_ne!(stale, live, "different generation by construction");

    let rejected = endpoint
        .cancel(stale)
        .expect_err("a stale generation must not cancel the live operation");
    assert_rejected_without_a_native_call(&rejected);
    assert_eq!(port.outstanding(), 1, "the live operation must survive");

    // A generation that was never issued is rejected too, so the check is an
    // equality test rather than an ordering test.
    let ahead = OperationId::from_parts(live.as_ptr(), live.generation() + 1);
    let rejected = endpoint
        .cancel(ahead)
        .expect_err("a generation that was never issued must be rejected");
    assert_rejected_without_a_native_call(&rejected);
    assert_eq!(port.outstanding(), 1);

    // The live identity still works, so the rejections disturbed nothing.
    endpoint.cancel(live).expect("the live identity must work");
    let completion = port.get(5_000).expect("get").expect("a completion");
    assert_eq!(completion.id(), Some(live));
    // SAFETY: matches the Operation<usize> submitted above, claimed once.
    let _operation = unsafe { completion.claim::<usize>() };
    assert_eq!(port.outstanding(), 0);
}

/// Cancelling with an identity whose operation has already completed is rejected
/// even when nothing else has taken its address.
#[test]
fn a_completed_operations_identity_is_rejected() {
    let path = temp_file_with(b"identity durability", "completed");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x52)
        .expect("associate");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let spent = spend_one_operation(&endpoint, &port, slot, 1);

    let rejected = endpoint
        .cancel(spent)
        .expect_err("a completed operation's identity must be rejected");
    assert_rejected_without_a_native_call(&rejected);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// An identity minted by one port must not be honored by another.
///
/// The operations are issued on pipes carrying no data so they stay genuinely
/// outstanding, which makes the rejection attributable to the identity check
/// rather than to the operation having already finished.
#[test]
fn an_identity_from_another_port_is_rejected() {
    let (pipe_a, _client_a) = pending_pipe("cross-a");
    let (pipe_b, _client_b) = pending_pipe("cross-b");

    let port_a = CompletionPort::new(0).expect("create port a");
    let endpoint_a = port_a.associate(pipe_a, 0x53).expect("associate a");
    let port_b = CompletionPort::new(0).expect("create port b");
    let endpoint_b = port_b.associate(pipe_b, 0x54).expect("associate b");

    let mut landed_a = 0_u8;
    let slot_a: *mut u8 = &mut landed_a;

    let operation = Operation::new(1_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed_a` outlives it because it is drained below.
    let submitted =
        unsafe { endpoint_a.submit(operation, |handle, ov| issue_read(handle, ov, slot_a)) };
    let id_a = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };
    assert_eq!(port_a.outstanding(), 1, "the read must be outstanding");

    // Port B has never seen this identity, so it must refuse to act on it.
    let rejected = endpoint_b
        .cancel(id_a)
        .expect_err("an identity from another port must be rejected");
    assert_rejected_without_a_native_call(&rejected);
    assert_eq!(port_a.outstanding(), 1, "port A's operation must survive");

    // Port A owns it, so its own identity still works.
    endpoint_a
        .cancel(id_a)
        .expect("the owning port must cancel");

    let completion = port_a.get(5_000).expect("get").expect("a completion");
    assert_eq!(completion.id(), Some(id_a));
    // SAFETY: matches the Operation<usize> submitted above, claimed once.
    let _operation = unsafe { completion.claim::<usize>() };
    assert_eq!(port_a.outstanding(), 0);
}

// --- identities remain usable for their intended purposes ---

/// A live identity still cancels its own operation, and the completion reports
/// the same identity that submission returned.
#[test]
fn a_live_identity_cancels_and_matches_its_completion() {
    let path = temp_file_with(b"identity durability", "live");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x55)
        .expect("associate");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(42_usize);
    // SAFETY: one 1-byte overlapped ReadFile into `landed`, drained below.
    let submitted =
        unsafe { endpoint.submit(operation, |handle, ov| issue_read(handle, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };

    let completion = port.get(5_000).expect("get").expect("a completion");
    assert_eq!(
        completion.id(),
        Some(id),
        "a completion must report the identity submission returned"
    );
    // SAFETY: matches the Operation<usize> submitted above, claimed once.
    let operation = unsafe { completion.claim::<usize>() };
    assert_eq!(*operation.payload(), 42);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// A live identity really does cancel its own operation -- the rejection of
/// stale identities must not have made cancellation useless.
#[test]
fn a_live_identity_still_cancels_a_genuinely_pending_operation() {
    /// `ERROR_OPERATION_ABORTED`. Changing this value is a breaking change.
    const ERROR_OPERATION_ABORTED: i32 = 995;

    let (pipe, _client) = pending_pipe("live-cancel");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port.associate(pipe, 0x57).expect("associate");

    let mut landed = 0_u8;
    let slot: *mut u8 = &mut landed;

    let operation = Operation::new(9_usize);
    // SAFETY: one 1-byte overlapped ReadFile on a pipe carrying no data, so it
    // stays pending; `landed` outlives it because it is drained below.
    let submitted =
        unsafe { endpoint.submit(operation, |handle, ov| issue_read(handle, ov, slot)) };
    let id = match submitted {
        Submitted::Pending(id) => id,
        other => panic!("expected pending, got {other:?}"),
    };
    assert_eq!(port.outstanding(), 1);

    endpoint
        .cancel(id)
        .expect("a live identity must cancel its own operation");

    let completion = port.get(5_000).expect("get").expect("a completion");
    assert_eq!(completion.id(), Some(id));
    assert_eq!(
        completion.error().and_then(|error| error.raw_os_error()),
        Some(ERROR_OPERATION_ABORTED),
        "the cancelled operation must complete as ERROR_OPERATION_ABORTED"
    );
    // SAFETY: matches the Operation<usize> submitted above, claimed once.
    let operation = unsafe { completion.claim::<usize>() };
    assert_eq!(*operation.payload(), 9);
    assert_eq!(port.outstanding(), 0);
}

/// A user packet completes no operation, so it carries no identity.
#[test]
fn a_user_packet_has_no_operation_identity() {
    let port = CompletionPort::new(0).expect("create port");
    port.post(0x99, 7).expect("post a user packet");

    let completion = port.get(5_000).expect("get").expect("a completion");
    assert_eq!(completion.key(), 0x99);
    assert_eq!(completion.bytes_transferred(), 7);
    assert_eq!(
        completion.id(),
        None,
        "a user packet completes no operation and has no identity"
    );
}

/// Identities of simultaneously outstanding operations are distinct, and each
/// completion matches exactly one of them.
#[test]
fn simultaneous_identities_match_their_own_completions() {
    const OPERATIONS: usize = 64;

    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "simultaneous");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x56)
        .expect("associate");

    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    let mut issued = HashSet::new();
    for slot in 0..OPERATIONS {
        let mut operation = Operation::new(slot);
        operation.set_offset(slot as u64);
        // SAFETY: one 1-byte overlapped ReadFile into this slot's own byte of
        // `landed`, which outlives every operation; all are drained below.
        let submitted = unsafe {
            endpoint.submit(operation, |handle, ov| {
                issue_read(handle, ov, base.add(slot))
            })
        };
        match submitted {
            Submitted::Pending(id) => {
                assert!(issued.insert(id), "slot {slot} reused a live identity");
            }
            other => panic!("expected pending at slot {slot}, got {other:?}"),
        }
    }

    let mut matched = HashSet::new();
    while port.outstanding() > 0 {
        let completion = port.get(5_000).expect("get").expect("a completion");
        let id = completion
            .id()
            .expect("an operation completion has an identity");
        assert!(
            issued.contains(&id),
            "completion reported an unknown identity"
        );
        assert!(matched.insert(id), "an identity completed twice");
        // SAFETY: matches the Operation<usize> submitted above, claimed once.
        let _operation = unsafe { completion.claim::<usize>() };
    }
    assert_eq!(matched.len(), OPERATIONS);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
