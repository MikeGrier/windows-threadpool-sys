// Copyright (c) 2026 Mike Grier
use crate::{
    BlockingEndpoint, CompletionPort, FILE_FLAG_NO_BUFFERING, PageBuffers, UnassociatedEndpoint,
};

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

#[test]
fn iocp_scatter_gather_via_token() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-sg-iocp-{}.tmp",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"").expect("create file");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, true, FILE_FLAG_NO_BUFFERING)
                .expect("open endpoint"),
            0,
        )
        .expect("associate");

    let mut src = PageBuffers::new(2);
    for (i, byte) in src.as_bytes_mut().iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }
    let expected: Vec<u8> = src.as_bytes().to_vec();

    // Gather-write, then dequeue and claim.
    let write_token = endpoint.write_gather(src, 0).expect("submit write_gather");
    let completion = port.get(5_000).expect("get").expect("write completion");
    let (returned, result) = match write_token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("write completion did not match its token"),
    };
    assert_eq!(result.expect("write result"), returned.len());
    assert_eq!(port.outstanding(), 0);

    // Scatter-read the same pages back.
    let read_token = endpoint.read_scatter(2, 0).expect("submit read_scatter");
    let completion = port.get(5_000).expect("get").expect("read completion");
    let (buffers, result) = match read_token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("read completion did not match its token"),
    };
    let read = result.expect("read result");
    assert_eq!(read, buffers.len());
    assert_eq!(buffers.as_bytes(), expected.as_slice());
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn blocking_scatter_gather_round_trips() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-sg-{}.tmp",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"").expect("create file");

    let endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, true, FILE_FLAG_NO_BUFFERING)
            .expect("open endpoint"),
    );

    let mut src = PageBuffers::new(2);
    for (i, byte) in src.as_bytes_mut().iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }

    let written = endpoint.write_gather(&src, 0).expect("write_gather");
    assert_eq!(written, src.len());

    let (dst, read) = endpoint.read_scatter(2, 0).expect("read_scatter");
    assert_eq!(read, src.len());
    assert_eq!(dst.as_bytes(), src.as_bytes());

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
