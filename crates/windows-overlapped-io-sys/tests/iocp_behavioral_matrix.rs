// Copyright (c) 2026 Mike Grier
//! Integration tests for the raw IOCP backend's behavioral matrix: synchronous
//! skip-on-success completion, completion identity under many simultaneous
//! operations, and results retained after native endpoint shutdown.

#![cfg(windows)]

use std::collections::HashMap;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;

use windows_overlapped_io_sys::{
    CompletionPort, Issued, Operation, OperationState, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::IO::OVERLAPPED;

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-iocp-matrix-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

fn open_overlapped(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open overlapped endpoint")
}

// Needs the `fs` feature, which is what carries the notification-mode setter.
#[cfg(feature = "fs")]
#[test]
fn skip_on_success_completes_synchronously_without_a_packet() {
    use windows_overlapped_io_sys::NotificationModes;

    let content = b"skip on success payload";
    let path = temp_file_with(content, "skip");

    // Set before association, which is the ordering `set_notification_modes`
    // documents: the flag is inert until the handle reaches a port, so there is
    // never a window in which an operation could be issued against a handle
    // whose notification behaviour is still undecided.
    let mut endpoint = open_overlapped(&path);
    endpoint
        .set_notification_modes(NotificationModes {
            skip_completion_port_on_success: true,
            ..NotificationModes::default()
        })
        .expect("a file handle supports skip-on-success");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port.associate(endpoint, 0x20).expect("associate");

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;
    let mut bytes: u32 = 0;
    let bytes_ptr: *mut u32 = &mut bytes;

    let mut operation = Operation::new(());
    operation.set_offset(0);
    // SAFETY: issues exactly one overlapped ReadFile into `buffer`. Under
    // skip-on-success a synchronous success writes `bytes` and queues no packet,
    // so it is reported as Completed; a pending read still delivers a packet.
    let submitted = unsafe {
        endpoint.submit(operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                buf_len,
                bytes_ptr,
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Completed {
                    bytes_transferred: *bytes_ptr,
                });
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(Issued::Pending)
            } else {
                Err(error)
            }
        })
    };

    match submitted {
        Submitted::Completed {
            operation,
            bytes_transferred,
        } => {
            // Synchronous path: no packet arrived and storage was reclaimed inline.
            assert_eq!(operation.state(), OperationState::Completed);
            assert_eq!(bytes_transferred as usize, content.len());
            assert_eq!(&buffer[..bytes_transferred as usize], content);
            assert_eq!(port.outstanding(), 0);
        }
        Submitted::Pending(id) => {
            // Pending path: a packet still arrives; drain and claim it.
            let completion = port.get(5_000).expect("get").expect("a completion");
            assert_eq!(completion.overlapped_ptr(), id.as_ptr());
            let read = completion.bytes_transferred() as usize;
            // SAFETY: matches the Operation<()> submitted above, claimed once.
            let operation = unsafe { completion.claim::<()>() };
            assert_eq!(operation.state(), OperationState::Completed);
            assert_eq!(read, content.len());
            assert_eq!(&buffer[..read], content);
            assert_eq!(port.outstanding(), 0);
        }
        Submitted::Failed { error, .. } => panic!("submit failed: {error}"),
    }

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn completion_identity_holds_under_many_simultaneous_operations() {
    const OPERATIONS: usize = 64;
    // A distinct byte per offset, so each read's landed byte identifies its slot.
    let content: Vec<u8> = (0..OPERATIONS).map(|i| i as u8).collect();
    let path = temp_file_with(&content, "identity");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x30)
        .expect("associate");

    // One byte of stable storage per operation; the Vec is never reallocated
    // after `base` is taken and outlives every claim.
    let mut landed = vec![0_u8; OPERATIONS];
    let base = landed.as_mut_ptr();

    let mut identities: HashMap<*mut OVERLAPPED, usize> = HashMap::new();
    for slot in 0..OPERATIONS {
        let mut operation = Operation::new(slot);
        operation.set_offset(slot as u64);
        // SAFETY: issues exactly one 1-byte overlapped ReadFile into this slot's
        // own byte within `landed`; slot < OPERATIONS and `landed` outlives the
        // operation.
        let submitted = unsafe {
            endpoint.submit(operation, |handle, overlapped| {
                let ok = ReadFile(
                    handle.as_raw_handle(),
                    base.add(slot),
                    1,
                    ptr::null_mut(),
                    overlapped,
                );
                if ok != 0 {
                    return Ok(Issued::Pending);
                }
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                    Ok(Issued::Pending)
                } else {
                    Err(error)
                }
            })
        };
        match submitted {
            Submitted::Pending(id) => {
                identities.insert(id.as_ptr(), slot);
            }
            Submitted::Completed { .. } => {
                panic!("skip mode not set; no inline completion expected")
            }
            Submitted::Failed { error, .. } => panic!("submit failed at slot {slot}: {error}"),
        }
    }
    assert_eq!(port.outstanding(), OPERATIONS);

    // Dequeue every completion and map it back to the operation that produced it.
    for _ in 0..OPERATIONS {
        let completion = port.get(5_000).expect("get").expect("a completion");
        let slot = *identities
            .get(&completion.overlapped_ptr())
            .expect("completion carries a known operation identity");
        assert!(completion.error().is_none());
        assert_eq!(completion.bytes_transferred(), 1);
        // SAFETY: this completion is the Operation<usize> submitted at `slot`.
        let operation = unsafe { completion.claim::<usize>() };
        assert_eq!(*operation.payload(), slot, "payload identity mismatch");
        assert_eq!(landed[slot], slot as u8, "wrong data landed in slot buffer");
    }
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn result_survives_endpoint_shutdown() {
    let content = b"result outlives the handle";
    let path = temp_file_with(content, "retain");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(open_overlapped(&path), 0x40)
        .expect("associate");

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;

    let mut operation = Operation::new(());
    operation.set_offset(0);
    // SAFETY: one overlapped ReadFile into `buffer`, which outlives the claim.
    let submitted = unsafe {
        endpoint.submit(operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                buf_len,
                ptr::null_mut(),
                overlapped,
            );
            if ok != 0 {
                return Ok(Issued::Pending);
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(Issued::Pending)
            } else {
                Err(error)
            }
        })
    };
    let id = match submitted {
        Submitted::Pending(id) => id,
        Submitted::Completed { .. } => panic!("skip mode not set; no inline completion expected"),
        Submitted::Failed { error, .. } => panic!("submit failed: {error}"),
    };

    // Observe the completion, then shut the endpoint down (closing the native
    // handle) before consuming the result.
    let completion = port.get(5_000).expect("get").expect("a completion");
    assert_eq!(completion.overlapped_ptr(), id.as_ptr());
    drop(endpoint);

    // The result and payload remain valid after the native handle is gone.
    let read = completion.bytes_transferred() as usize;
    // SAFETY: matches the Operation<()> submitted above, claimed exactly once.
    let operation = unsafe { completion.claim::<()>() };
    assert_eq!(operation.state(), OperationState::Completed);
    assert_eq!(read, content.len());
    assert_eq!(&buffer[..read], content);
    assert_eq!(port.outstanding(), 0);

    let _ = std::fs::remove_file(&path);
}
