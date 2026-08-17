// Copyright (c) 2026 Mike Grier
use crate::{BlockingEndpoint, CompletionPort, UnassociatedEndpoint};

#[test]
fn blocking_write_then_read_round_trips() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-blocking-{}.tmp",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"").expect("create file");

    let endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, true, 0).expect("open endpoint"),
    );

    let data = b"safe file adapter round trip";
    let written = endpoint.write(data, 0).expect("write");
    assert_eq!(written, data.len());

    let (buffer, read) = endpoint.read(data.len(), 0).expect("read");
    assert_eq!(read, data.len());
    assert_eq!(buffer, data);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn iocp_read_via_file_io_token() {
    let content = b"iocp file adapter content";
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-iocp-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write file");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    let token = endpoint.read(content.len(), 0).expect("submit read");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (buffer, result) = match token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("completion did not match the token"),
    };
    let read = result.expect("read result");
    assert_eq!(read, content.len());
    assert_eq!(&buffer[..read], content);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
