// Copyright (c) 2026 Mike Grier
use super::{AssociatedEndpoint, CompletionPort, Issued, Submitted};
use crate::{Operation, OperationState, UnassociatedEndpoint};
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::OwnedHandle;
use std::path::PathBuf;

const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

fn associate_temp_file<'port>(
    port: &'port CompletionPort,
    tag: &str,
) -> (AssociatedEndpoint<'port>, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-{tag}-{}.tmp",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("create overlapped temp file");
    let owned = OwnedHandle::from(file);
    // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
    // associated with any port, has no duplicates, and moves in exclusively.
    let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    let associated = port.associate(endpoint, 0).expect("associate");
    (associated, path)
}

#[test]
fn posts_and_dequeues_a_user_packet() {
    let port = CompletionPort::new(0).expect("create port");

    port.post(0xABCD, 42).expect("post packet");
    let completion = port.get(1_000).expect("get packet").expect("a packet");

    assert_eq!(completion.key(), 0xABCD);
    assert_eq!(completion.bytes_transferred(), 42);
    assert!(completion.overlapped_ptr().is_null());
    assert!(completion.error().is_none());
}

#[test]
fn get_times_out_when_empty() {
    let port = CompletionPort::new(0).expect("create port");
    assert!(port.get(0).expect("get").is_none());
}

#[test]
fn associates_an_overlapped_handle() {
    let port = CompletionPort::new(0).expect("create port");
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-iocp-{}.tmp",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("create overlapped temp file");
    let owned = OwnedHandle::from(file);

    // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
    // associated with any port, has no duplicates, and moves in exclusively.
    let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    let associated = port.associate(endpoint, 0x55).expect("associate");
    assert_eq!(associated.key(), 0x55);

    drop(associated);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn submit_pending_then_claim_recovers_the_operation() {
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = associate_temp_file(&port, "submit-pending");

    let operation = Operation::new(vec![1_u8, 2, 3]);
    // SAFETY: the closure issues exactly one operation using the given
    // OVERLAPPED pointer; here it simulates a device that queues a
    // completion for that pointer, so a packet will arrive.
    let submitted = unsafe {
        endpoint.submit(operation, |_handle, overlapped| {
            port.post_raw(7, 3, overlapped)?;
            Ok(Issued::Pending)
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));
    let Submitted::Pending(id) = submitted else {
        unreachable!("just asserted pending");
    };
    assert_eq!(port.outstanding(), 1);

    let completion = port.get(1_000).expect("get").expect("a packet");
    assert_eq!(completion.overlapped_ptr(), id.as_ptr());
    // SAFETY: this completion is from the Operation<Vec<u8>> submitted above
    // and is claimed exactly once.
    let operation = unsafe { completion.claim::<Vec<u8>>() };
    assert_eq!(operation.state(), OperationState::Completed);
    assert_eq!(operation.payload(), &vec![1_u8, 2, 3]);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn submit_immediate_failure_returns_the_operation() {
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = associate_temp_file(&port, "submit-fail");

    let operation = Operation::new(vec![9_u8]);
    // SAFETY: the closure issues no operation and reports an immediate
    // failure, so no completion will arrive.
    let submitted = unsafe {
        endpoint.submit(operation, |_handle, _overlapped| {
            Err(std::io::Error::from_raw_os_error(5))
        })
    };
    match submitted {
        Submitted::Failed { operation, error } => {
            assert_eq!(operation.payload(), &vec![9_u8]);
            assert_eq!(operation.state(), OperationState::Idle);
            assert_eq!(error.raw_os_error(), Some(5));
        }
        Submitted::Completed { .. } => panic!("expected immediate failure"),
        Submitted::Pending(_) => panic!("expected immediate failure"),
    }
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn unclaimed_completion_reclaims_on_drop() {
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = associate_temp_file(&port, "unclaimed");

    let operation = Operation::new(vec![7_u8]);
    // SAFETY: the closure issues one operation using the given OVERLAPPED
    // pointer; here it simulates a device that queues its completion.
    let submitted = unsafe {
        endpoint.submit(operation, |_handle, overlapped| {
            port.post_raw(0, 1, overlapped)?;
            Ok(Issued::Pending)
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));
    assert_eq!(port.outstanding(), 1);

    // Drop the completion without claiming it; its storage is reclaimed.
    let completion = port.get(1_000).expect("get").expect("a packet");
    drop(completion);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// Dequeuing removes the operation from the outstanding set. The completion
/// still owns the storage, but the port is no longer waiting to deliver
/// anything for it, which is what `outstanding` measures.
#[test]
fn dequeuing_clears_the_outstanding_count_even_while_held() {
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = associate_temp_file(&port, "held-outstanding");

    let operation = Operation::new(vec![7_u8]);
    // SAFETY: the closure issues one operation using the given OVERLAPPED
    // pointer; here it simulates a device that queues its completion.
    let submitted = unsafe {
        endpoint.submit(operation, |_handle, overlapped| {
            port.post_raw(0, 1, overlapped)?;
            Ok(Issued::Pending)
        })
    };
    assert!(matches!(submitted, Submitted::Pending(_)));
    assert_eq!(port.outstanding(), 1, "queued, not yet delivered");

    let completion = port.get(1_000).expect("get").expect("a packet");
    assert_eq!(
        port.outstanding(),
        0,
        "the packet has been delivered, so nothing is outstanding while it is held"
    );

    drop(completion);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// Dropping the port while a completion is still held must not block. The
/// packet has already left the queue, so counting it as outstanding made
/// `run_down` wait in `get(INFINITE)` for a packet that could never arrive --
/// an unconditional hang reachable from entirely safe code.
///
/// The work runs on a thread so a regression is a test failure rather than a
/// wedged test run.
#[test]
fn dropping_the_port_while_a_completion_is_held_does_not_hang() {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let port = CompletionPort::new(0).expect("create port");
        let (endpoint, path) = associate_temp_file(&port, "held-port-drop");

        let operation = Operation::new(vec![9_u8]);
        // SAFETY: the closure issues one operation using the given OVERLAPPED
        // pointer; here it simulates a device that queues its completion.
        let submitted = unsafe {
            endpoint.submit(operation, |_handle, overlapped| {
                port.post_raw(1, 1, overlapped)?;
                Ok(Issued::Pending)
            })
        };
        assert!(matches!(submitted, Submitted::Pending(_)));

        // Dequeue but neither claim nor drop: the completion outlives the port.
        let completion = port.get(1_000).expect("get").expect("a packet");
        drop(endpoint);
        drop(port);

        // The held completion still owns the storage and frees it here, after
        // the port it came from is already gone.
        drop(completion);
        let _ = std::fs::remove_file(&path);
        let _ = tx.send(());
    });

    rx.recv_timeout(std::time::Duration::from_secs(20))
        .expect("dropping the port with a completion held must not block");
}

#[test]
fn synchronous_completion_reclaims_inline_without_a_packet() {
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = associate_temp_file(&port, "sync-complete");

    let operation = Operation::new(vec![5_u8, 6, 7]);
    // SAFETY: the closure starts no real operation and reports a synchronous
    // completion, so no packet will arrive for this OVERLAPPED and reclaiming
    // its storage inline is sound.
    let submitted = unsafe {
        endpoint.submit(operation, |_handle, _overlapped| {
            Ok(Issued::Completed {
                bytes_transferred: 3,
            })
        })
    };
    match submitted {
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => {
            assert_eq!(bytes_transferred, 3);
            assert_eq!(operation.state(), OperationState::Completed);
            assert_eq!(operation.payload(), &vec![5_u8, 6, 7]);
        }
        _ => panic!("expected synchronous completion"),
    }
    // No packet is outstanding, so the port needs no drain before teardown.
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
