// Copyright (c) 2026 Mike Grier
//! Integration test (`fs` feature): safe file write-then-read round-trips on
//! both the blocking and IOCP backends, with no `unsafe` in the test's I/O path.

#![cfg(all(windows, feature = "fs"))]

use std::path::PathBuf;

use windows_overlapped_io_sys::{BlockingEndpoint, CompletionPort, UnassociatedEndpoint};

fn empty_temp_file(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-int-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"").expect("create temp file");
    path
}

#[test]
fn blocking_backend_round_trips_a_file() {
    let path = empty_temp_file("blocking");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, true, 0).expect("open endpoint"),
    );

    let data = b"blocking safe adapter round trip";
    let written = endpoint.write(data, 0).expect("write");
    assert_eq!(written, data.len());

    let (buffer, read) = endpoint.read(data.len(), 0).expect("read");
    assert_eq!(read, data.len());
    assert_eq!(buffer, data);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn iocp_backend_round_trips_a_file() {
    let path = empty_temp_file("iocp");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, true, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    let data = b"iocp safe adapter round trip".to_vec();

    // Write, then dequeue and claim its completion via the token.
    let write_token = endpoint
        .write(data.clone(), 0)
        .expect("submit write")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("write completion");
    let (returned, result) = write_token
        .claim(&completion)
        .unwrap_or_else(|_| panic!("write completion did not match its token"));
    assert_eq!(result.expect("write result"), data.len());
    assert_eq!(returned, data);
    assert_eq!(port.outstanding(), 0);

    // Read it back the same way.
    let read_token = endpoint
        .read(vec![0_u8; data.len()], 0)
        .expect("submit read")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("read completion");
    let (buffer, result) = read_token
        .claim(&completion)
        .unwrap_or_else(|_| panic!("read completion did not match its token"));
    let read = result.expect("read result");
    assert_eq!(read, data.len());
    assert_eq!(&buffer[..read], data.as_slice());
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
