// Copyright (c) 2026 Mike Grier
use crate::{BlockingEndpoint, UnassociatedEndpoint};

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
