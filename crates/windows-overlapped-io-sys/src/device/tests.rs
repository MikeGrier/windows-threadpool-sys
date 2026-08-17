// Copyright (c) 2026 Mike Grier
use crate::{BlockingEndpoint, CompletionPort, UnassociatedEndpoint};
use windows_sys::Win32::System::Ioctl::FSCTL_GET_COMPRESSION;

fn temp_file(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-ioctl-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"device control test").expect("create file");
    path
}

#[test]
fn blocking_ioctl_get_compression() {
    let path = temp_file("blocking");
    let endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
    );

    // FSCTL_GET_COMPRESSION returns a USHORT compression state (2 bytes).
    let (output, returned) = endpoint
        .ioctl(FSCTL_GET_COMPRESSION, &[], 2)
        .expect("ioctl");
    assert_eq!(returned, 2);
    assert_eq!(output.len(), 2);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn iocp_ioctl_get_compression() {
    let path = temp_file("iocp");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    let token = endpoint
        .ioctl(FSCTL_GET_COMPRESSION, Vec::new(), 2)
        .expect("submit ioctl");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (output, result) = match token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("completion did not match its token"),
    };
    let returned = result.expect("ioctl result");
    assert_eq!(returned, 2);
    assert!(output.len() >= 2);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
