// Copyright (c) 2026 Mike Grier
//! Integration test (`fs` feature): page-aligned gather-write-then-scatter-read
//! round-trips on both the blocking and IOCP backends, with no `unsafe` in the
//! test's I/O path.

#![cfg(all(windows, feature = "fs"))]

use std::path::PathBuf;

use windows_overlapped_io_sys::{
    BlockingEndpoint, CompletionPort, FILE_FLAG_NO_BUFFERING, PageBuffers, UnassociatedEndpoint,
};

const PAGES: usize = 16;

fn empty_temp_file(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-sg-int-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"").expect("create temp file");
    path
}

fn filled_pages(pages: usize) -> PageBuffers {
    let mut buffers = PageBuffers::new(pages);
    for (i, byte) in buffers.as_bytes_mut().iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }
    buffers
}

#[test]
fn blocking_backend_scatter_gather_round_trips() {
    let path = empty_temp_file("blocking");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, true, FILE_FLAG_NO_BUFFERING)
            .expect("open endpoint"),
    );

    let src = filled_pages(PAGES);
    let written = endpoint.write_gather(&src, 0).expect("write_gather");
    assert_eq!(written, src.len());

    let (dst, read) = endpoint.read_scatter(PAGES, 0).expect("read_scatter");
    assert_eq!(read, src.len());
    assert_eq!(dst.as_bytes(), src.as_bytes());

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn iocp_backend_scatter_gather_round_trips() {
    let path = empty_temp_file("iocp");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, true, FILE_FLAG_NO_BUFFERING)
                .expect("open endpoint"),
            0,
        )
        .expect("associate");

    let src = filled_pages(PAGES);
    let expected = src.as_bytes().to_vec();

    // Gather-write, then dequeue and claim via the token.
    let write_token = endpoint
        .write_gather(src, 0)
        .expect("submit write_gather")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("write completion");
    let (returned, result) = write_token
        .claim(&completion)
        .unwrap_or_else(|_| panic!("write completion did not match its token"));
    assert_eq!(result.expect("write result"), returned.len());
    assert_eq!(port.outstanding(), 0);

    // Scatter-read the same pages back.
    let read_token = endpoint
        .read_scatter(PAGES, 0)
        .expect("submit read_scatter")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("read completion");
    let (buffers, result) = read_token
        .claim(&completion)
        .unwrap_or_else(|_| panic!("read completion did not match its token"));
    let read = result.expect("read result");
    assert_eq!(read, buffers.len());
    assert_eq!(buffers.as_bytes(), expected.as_slice());
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
