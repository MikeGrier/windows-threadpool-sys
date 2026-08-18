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

// --- buffer length limits ---

/// The byte counts `DeviceIoControl` takes are `u32`, so a longer buffer cannot
/// be described to it. Capping would submit a prefix of the caller's input, or
/// describe the output buffer as smaller than it is, and then report success for
/// an operation that did something other than what was asked.
#[test]
fn checked_len_rejects_lengths_beyond_u32() {
    use crate::device::checked_len;

    assert!(checked_len(0, "input").is_ok());
    assert_eq!(checked_len(1024, "input").expect("fits"), 1024);
    assert_eq!(
        checked_len(u32::MAX as usize, "input").expect("the largest fitting length"),
        u32::MAX
    );

    #[cfg(target_pointer_width = "64")]
    {
        let too_long = u32::MAX as usize + 1;
        let error = checked_len(too_long, "input").expect_err("must not cap");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            error.to_string().contains("input"),
            "the error should name the offending buffer: {error}"
        );
        assert!(
            checked_len(too_long, "output").is_err(),
            "the output buffer has the same limit"
        );
    }
}

/// The output length is checked before the buffer is allocated, so an
/// unrepresentable request costs nothing -- which is also what makes this test
/// affordable: it never allocates the 4GiB it asks for.
#[cfg(target_pointer_width = "64")]
#[test]
fn blocking_ioctl_rejects_an_oversized_output_buffer() {
    let path = temp_file("oversized-output");
    let endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
    );

    let error = endpoint
        .ioctl(FSCTL_GET_COMPRESSION, &[], u32::MAX as usize + 1)
        .expect_err("an unrepresentable output length must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the request should be rejected before reaching the kernel: {error}"
    );

    let _ = std::fs::remove_file(&path);
}

/// The same check on the submitting path, which measures the lengths before
/// building the operation because its submission closure runs at the FFI
/// boundary and has no way to report an error.
#[cfg(target_pointer_width = "64")]
#[test]
fn submitted_ioctl_rejects_an_oversized_output_buffer() {
    let path = temp_file("oversized-output-submit");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate endpoint");

    let error = endpoint
        .ioctl(FSCTL_GET_COMPRESSION, Vec::new(), u32::MAX as usize + 1)
        .expect_err("an unrepresentable output length must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the request should be rejected before reaching the kernel: {error}"
    );

    let _ = std::fs::remove_file(&path);
}
