// Copyright (c) 2026 Mike Grier
//! Integration test (`device` feature): an `FSCTL` query on a real file through
//! both the blocking and IOCP `ioctl` adapters. `ioctl` is `unsafe` because it
//! takes an arbitrary control code; `FSCTL_GET_COMPRESSION` is self-contained,
//! which is what the `unsafe` blocks below assert.

#![cfg(all(windows, feature = "device"))]

use std::path::PathBuf;

use windows_overlapped_io_sys::{BlockingEndpoint, CompletionPort, UnassociatedEndpoint};
use windows_sys::Win32::System::Ioctl::FSCTL_GET_COMPRESSION;

/// `FSCTL_GET_COMPRESSION` returns a `USHORT` compression state.
const COMPRESSION_STATE_LEN: usize = 2;

fn temp_file(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-ioctl-int-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"integration device control").expect("create temp file");
    path
}

#[test]
fn blocking_backend_queries_compression() {
    let path = temp_file("blocking");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
    );

    // SAFETY: FSCTL_GET_COMPRESSION is self-contained -- empty input, and it
    // writes only the owned output buffer, embedding no pointers.
    let (output, returned) =
        unsafe { endpoint.ioctl(FSCTL_GET_COMPRESSION, &[], COMPRESSION_STATE_LEN) }
            .expect("ioctl");
    assert_eq!(returned, COMPRESSION_STATE_LEN);
    assert_eq!(output.len(), COMPRESSION_STATE_LEN);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn iocp_backend_queries_compression() {
    let path = temp_file("iocp");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    // SAFETY: FSCTL_GET_COMPRESSION is self-contained -- empty input, and it
    // writes only the owned output buffer, embedding no pointers.
    let token = unsafe { endpoint.ioctl(FSCTL_GET_COMPRESSION, Vec::new(), COMPRESSION_STATE_LEN) }
        .expect("submit ioctl");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (output, result) = token
        .claim(&completion)
        .unwrap_or_else(|_| panic!("completion did not match its token"));
    let returned = result.expect("ioctl result");
    assert_eq!(returned, COMPRESSION_STATE_LEN);
    assert!(output.len() >= COMPRESSION_STATE_LEN);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
