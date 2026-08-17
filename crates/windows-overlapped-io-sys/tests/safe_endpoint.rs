// Copyright (c) 2026 Mike Grier
//! Integration test: an endpoint created by the safe `UnassociatedEndpoint::open`
//! creator runs a real `ReadFile` on both the blocking and IOCP backends, with
//! no `assume_overlapped` in sight.

#![cfg(windows)]

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::ptr;

use windows_overlapped_io_sys::{
    BlockingEndpoint, CompletionPort, Issued, Operation, OperationState, Submitted,
    UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::ReadFile;

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-safe-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

#[test]
fn safe_created_endpoint_reads_on_the_blocking_backend() {
    let content = b"safe blocking read";
    let path = temp_file_with(content, "blocking");

    let endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
    );

    let mut operation = Operation::new(());
    operation.set_offset(0);
    let mut buffer = [0_u8; 64];
    // SAFETY: issues exactly one overlapped ReadFile into `buffer`, which stays
    // valid for the whole blocking call; no other operation is outstanding.
    let read = unsafe {
        endpoint.run(&mut operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                ptr::null_mut(),
                overlapped,
            );
            if ok != 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                Ok(())
            } else {
                Err(error)
            }
        })
    }
    .expect("run read");

    assert_eq!(read, content.len());
    assert_eq!(&buffer[..read], content);
    assert_eq!(operation.state(), OperationState::Completed);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn safe_created_endpoint_reads_on_the_iocp_backend() {
    let content = b"safe iocp read";
    let path = temp_file_with(content, "iocp");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0x10,
        )
        .expect("associate");

    let mut buffer = [0_u8; 64];
    let buf_ptr = buffer.as_mut_ptr();
    let buf_len = buffer.len() as u32;

    let mut operation = Operation::new(());
    operation.set_offset(0);
    // SAFETY: issues exactly one overlapped ReadFile using the given OVERLAPPED
    // pointer; `buffer` outlives the operation until its completion is claimed.
    let submitted = unsafe {
        endpoint.submit(operation, |handle, overlapped| {
            let ok = ReadFile(
                handle.as_raw_handle(),
                buf_ptr,
                buf_len,
                ptr::null_mut(),
                overlapped,
            );
            // Skip-on-success mode is not set on this handle, so even a
            // synchronous success queues a packet: report Pending either way.
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
        Submitted::Completed { .. } => {
            panic!("unexpected synchronous completion without skip mode")
        }
        Submitted::Failed { error, .. } => panic!("submit failed: {error}"),
    };

    let completion = port.get(5_000).expect("get").expect("a completion");
    assert_eq!(completion.overlapped_ptr(), id.as_ptr());
    assert!(completion.error().is_none());
    let read = completion.bytes_transferred() as usize;
    // SAFETY: this completion is from the Operation<()> submitted above and is
    // claimed exactly once.
    let operation = unsafe { completion.claim::<()>() };
    assert_eq!(operation.state(), OperationState::Completed);
    assert_eq!(port.outstanding(), 0);

    assert_eq!(read, content.len());
    assert_eq!(&buffer[..read], content);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
