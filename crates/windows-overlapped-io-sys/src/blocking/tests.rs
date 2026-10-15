// Copyright (c) 2026 Mike Grier
use std::io::{self, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::mpsc;

use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE};

use crate::{BlockingEndpoint, NotificationModes, Operation, UnassociatedEndpoint};

fn temp_file(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-blocking-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("create file");
    path
}

/// An overlapped pipe server endpoint with a connected client, so a read on the
/// server pends until the client writes.
fn connected_pipe(tag: &str) -> (UnassociatedEndpoint, std::fs::File) {
    let name = format!(
        r"\\.\pipe\windows-overlapped-io-sys-blocking-{tag}-{}",
        std::process::id()
    );
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a NUL-terminated name and no security attributes.
    let raw = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE,
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert_ne!(
        raw,
        INVALID_HANDLE_VALUE,
        "CreateNamedPipeW: {}",
        io::Error::last_os_error()
    );
    // SAFETY: a fresh overlapped pipe, unassociated, unduplicated, moved in exclusively.
    let endpoint =
        unsafe { UnassociatedEndpoint::assume_overlapped(OwnedHandle::from_raw_handle(raw)) };
    let client = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .expect("connect the client end");
    (endpoint, client)
}

/// Read into `buffer` through `run`, calling `pended` once the read has gone pending.
fn read_reporting_pending(
    endpoint: &BlockingEndpoint,
    buffer: &mut [u8],
    pended: impl FnOnce(),
) -> io::Result<usize> {
    let mut operation = Operation::new(());
    let buf_ptr = buffer.as_mut_ptr();
    let len = u32::try_from(buffer.len()).expect("small buffer");
    // SAFETY: one overlapped ReadFile into `buffer`, which outlives this blocking call.
    unsafe {
        endpoint.run(&mut operation, |handle, overlapped| {
            if ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                len,
                std::ptr::null_mut(),
                overlapped,
            ) != 0
            {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            pended();
            Ok(())
        })
    }
}

#[test]
fn accepts_an_endpoint_with_no_notification_mode() {
    let path = temp_file("plain");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    assert!(BlockingEndpoint::new(endpoint).is_ok());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn accepts_an_endpoint_with_every_notification_mode() {
    let path = temp_file("all-modes");
    let mut endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    endpoint
        .set_notification_modes(NotificationModes {
            skip_completion_port_on_success: true,
            skip_set_event_on_handle: true,
        })
        .expect("set notification modes");
    let blocking = BlockingEndpoint::new(endpoint).expect("every mode is accepted");

    let mut buffer = [0_u8; 4];
    let read = read_reporting_pending(&blocking, &mut buffer, || {}).expect("read");
    assert_eq!(&buffer[..read], b"x");
    drop(blocking);
    let _ = std::fs::remove_file(&path);
}

/// The wait must not use the handle's own event: with that event suppressed, a
/// genuinely pending read completes only if `run` waits on an event of its own.
#[test]
fn a_pending_read_completes_with_the_handles_event_suppressed() {
    let (mut endpoint, mut client) = connected_pipe("skip-event");
    endpoint
        .set_notification_modes(NotificationModes {
            skip_set_event_on_handle: true,
            ..NotificationModes::default()
        })
        .expect("set notification modes");
    let blocking = BlockingEndpoint::new(endpoint).expect("blocking endpoint");

    let (pended, writer_go) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        writer_go.recv().expect("the read pended");
        client.write_all(b"late").expect("client write");
        client
    });
    let mut buffer = [0_u8; 4];
    let read = read_reporting_pending(&blocking, &mut buffer, || {
        pended.send(()).expect("wake the writer");
    })
    .expect("read");
    assert_eq!(&buffer[..read], b"late");
    drop(writer.join().expect("writer"));
}

/// An observer's queries reset the handle's event (measured below); a pending
/// read must still complete when they run while it is in flight.
#[cfg(feature = "fs")]
#[test]
fn observer_queries_while_a_read_pends_do_not_disturb_it() {
    let (endpoint, mut client) = connected_pipe("observer-queries");
    let blocking = BlockingEndpoint::new(endpoint).expect("blocking endpoint");
    let observer = blocking.observer();

    let (pended, writer_go) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        writer_go.recv().expect("the read pended");
        for _ in 0..16 {
            observer.file_len().expect("a query while the read pends");
        }
        client.write_all(b"data").expect("client write");
        for _ in 0..16 {
            observer.file_len().expect("a query as the read completes");
        }
        client
    });
    let mut buffer = [0_u8; 4];
    let read = read_reporting_pending(&blocking, &mut buffer, || {
        pended.send(()).expect("wake the writer");
    })
    .expect("read");
    assert_eq!(&buffer[..read], b"data");
    drop(writer.join().expect("writer"));
}

/// Every observer query, not only `file_len`, while a read pends (M17.10). The
/// blocking backend has no cancellation, so completion is the only outcome.
#[cfg(feature = "fs")]
#[test]
fn every_query_while_a_read_pends_leaves_it_to_complete() {
    use crate::test_util::{OBSERVER_QUERIES, run_every_query};

    let (endpoint, mut client) = connected_pipe("every-query");
    let blocking = BlockingEndpoint::new(endpoint).expect("blocking endpoint");
    let observer = blocking.observer();

    let (pended, writer_go) = mpsc::channel::<()>();
    let writer = std::thread::spawn(move || {
        writer_go.recv().expect("the read pended");
        for _ in 0..4 {
            assert_eq!(run_every_query(&observer).len(), OBSERVER_QUERIES.len());
        }
        client.write_all(b"data").expect("client write");
        assert_eq!(run_every_query(&observer).len(), OBSERVER_QUERIES.len());
        client
    });
    let mut buffer = [0_u8; 4];
    let read = read_reporting_pending(&blocking, &mut buffer, || {
        pended.send(()).expect("wake the writer");
    })
    .expect("read");
    assert_eq!(&buffer[..read], b"data");
    drop(writer.join().expect("writer"));
}

/// Pins the measurement M16.6 rests on: a synchronous query through an observer
/// leaves a signalled handle unsignalled.
#[cfg(feature = "fs")]
#[test]
fn an_observer_query_resets_the_handles_own_event() {
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    let path = temp_file("handle-event");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    let observer = endpoint.observer();
    let handle = endpoint.handle().as_raw_handle();

    let mut buffer = [0_u8; 1];
    let mut operation = Operation::new(());
    let mut transferred = 0_u32;
    // SAFETY: one overlapped ReadFile into `buffer` with no hEvent, waited for
    // on the handle before either goes out of scope.
    let done = unsafe {
        let ok = ReadFile(
            handle,
            buffer.as_mut_ptr(),
            1,
            std::ptr::null_mut(),
            operation.overlapped_ptr(),
        );
        assert!(
            ok != 0 || io::Error::last_os_error().raw_os_error() == Some(ERROR_IO_PENDING as i32),
            "ReadFile failed"
        );
        windows_sys::Win32::System::IO::GetOverlappedResult(
            handle,
            operation.overlapped_ptr(),
            &raw mut transferred,
            1,
        )
    };
    assert_ne!(done, 0, "the read completes");
    operation.set_state(crate::OperationState::Completed);

    // SAFETY: a live handle, polled without waiting.
    assert_eq!(
        unsafe { WaitForSingleObject(handle, 0) },
        WAIT_OBJECT_0,
        "completion signals the handle"
    );
    observer.file_len().expect("file_len");
    // SAFETY: as above.
    assert_eq!(
        unsafe { WaitForSingleObject(handle, 0) },
        WAIT_TIMEOUT,
        "the query resets it"
    );

    drop((observer, endpoint));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn run_lends_its_event_only_for_the_call() {
    let path = temp_file("lend");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    let blocking = BlockingEndpoint::new(endpoint).expect("blocking endpoint");
    let mut operation = Operation::new(());

    let mut seen = std::ptr::null_mut();
    let source = *b"no";
    // SAFETY: an overlapped WriteFile on a read-only handle, which fails at
    // once and starts nothing.
    let result = unsafe {
        blocking.run(&mut operation, |handle, overlapped| {
            seen = (*overlapped).hEvent;
            if WriteFile(
                handle.as_raw_handle(),
                source.as_ptr(),
                2,
                std::ptr::null_mut(),
                overlapped,
            ) != 0
            {
                return Ok(());
            }
            Err(io::Error::last_os_error())
        })
    };
    assert!(result.is_err(), "a write on a read-only handle fails");
    assert!(!seen.is_null(), "the call sees the endpoint's event");
    assert_ne!(
        seen,
        blocking.handle().as_raw_handle(),
        "an event, not the file handle"
    );
    // SAFETY: the operation is idle; nothing else holds its OVERLAPPED.
    assert!(
        unsafe { (*operation.overlapped_ptr()).hEvent }.is_null(),
        "taken back on the error path"
    );

    let mut buffer = [0_u8; 1];
    let buf_ptr = buffer.as_mut_ptr();
    // SAFETY: one overlapped ReadFile into `buffer`, which outlives this blocking call.
    let read = unsafe {
        blocking.run(&mut operation, |handle, overlapped| {
            if ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                1,
                std::ptr::null_mut(),
                overlapped,
            ) != 0
            {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(())
            } else {
                Err(error)
            }
        })
    };
    assert_eq!(read.expect("read"), 1);
    // SAFETY: as above.
    assert!(
        unsafe { (*operation.overlapped_ptr()).hEvent }.is_null(),
        "taken back on success"
    );

    drop(blocking);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn sequential_reads_reuse_the_event() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-blocking-sequential-{}.tmp",
        std::process::id()
    ));
    let content: Vec<u8> = (0..=255_u8).collect();
    std::fs::write(&path, &content).expect("write file");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    let blocking = BlockingEndpoint::new(endpoint).expect("blocking endpoint");

    for offset in [0_u32, 1, 17, 128, 255] {
        let mut operation = Operation::new(());
        operation.set_offset(u64::from(offset));
        let mut byte = [0_u8; 1];
        let buf_ptr = byte.as_mut_ptr();
        // SAFETY: one overlapped ReadFile into `byte`, which outlives this blocking call.
        let read = unsafe {
            blocking.run(&mut operation, |handle, overlapped| {
                if ReadFile(
                    handle.as_raw_handle(),
                    buf_ptr,
                    1,
                    std::ptr::null_mut(),
                    overlapped,
                ) != 0
                {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                    Ok(())
                } else {
                    Err(error)
                }
            })
        };
        assert_eq!(read.expect("read"), 1);
        assert_eq!(byte[0], content[offset as usize], "offset {offset}");
    }

    drop(blocking);
    let _ = std::fs::remove_file(&path);
}
