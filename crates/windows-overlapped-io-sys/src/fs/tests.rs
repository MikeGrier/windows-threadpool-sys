// Copyright (c) 2026 Mike Grier
use crate::{
    BlockingEndpoint, CompletionPort, FILE_FLAG_NO_BUFFERING, PAGE_SIZE, PageBuffers,
    UnassociatedEndpoint,
};

#[test]
fn blocking_write_then_read_round_trips() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-blocking-{}.tmp",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    std::fs::write(&path, b"").expect("create file");

    let mut endpoint = BlockingEndpoint::new(
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

    let token = endpoint
        .read(content.len(), 0)
        .expect("submit read")
        .expect_pending("this endpoint is not in skip-on-success mode");
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
    let write_token = endpoint
        .write_gather(src, 0)
        .expect("submit write_gather")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("write completion");
    let (returned, result) = match write_token.claim(&completion) {
        Ok(pair) => pair,
        Err(_) => panic!("write completion did not match its token"),
    };
    assert_eq!(result.expect("write result"), returned.len());
    assert_eq!(port.outstanding(), 0);

    // Scatter-read the same pages back.
    let read_token = endpoint
        .read_scatter(2, 0)
        .expect("submit read_scatter")
        .expect_pending("this endpoint is not in skip-on-success mode");
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

    let mut endpoint = BlockingEndpoint::new(
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

// --- buffer length limits ---

/// The byte counts `ReadFile`, `WriteFile` and the scatter/gather calls take are
/// `u32`, so a longer buffer cannot be described to them. Capping would transfer
/// a prefix and report success, which is the defect `checked_len` replaced.
#[test]
fn checked_len_rejects_lengths_beyond_u32() {
    use crate::fs::checked_len;

    assert_eq!(checked_len(0, "read buffer").expect("empty fits"), 0);
    assert_eq!(
        checked_len(u32::MAX as usize, "read buffer").expect("the largest fitting length"),
        u32::MAX
    );

    #[cfg(target_pointer_width = "64")]
    {
        let too_long = u32::MAX as usize + 1;
        let error = checked_len(too_long, "read buffer").expect_err("must not cap");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            error.to_string().contains("read buffer"),
            "the error should name the offending buffer: {error}"
        );
    }
}

/// A page count whose byte total overflows `usize` must be an error, not a
/// panic. This is checked rather than saturating multiplication precisely
/// because on 32-bit Windows `usize::MAX` *is* `u32::MAX`, so saturating would
/// produce a value the length check accepts and `PageBuffers::new` would then
/// panic on its own checked multiplication.
#[test]
fn an_overflowing_page_count_is_rejected() {
    use crate::fs::scatter_gather_len;

    let error = scatter_gather_len(usize::MAX).expect_err("overflow must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);

    // Just past the largest total that fits, on any pointer width.
    let overflowing = (usize::MAX / crate::fs::PAGE_SIZE) + 1;
    assert!(
        scatter_gather_len(overflowing).is_err(),
        "a page count whose total overflows must be rejected"
    );

    // And a representable one still works.
    assert_eq!(
        scatter_gather_len(1).expect("one page fits"),
        crate::fs::PAGE_SIZE as u32
    );
}

/// A zero page count is rejected before it reaches `PageBuffers::new`, which
/// panics on zero, so the scatter adapters return `InvalidInput` on a degenerate
/// request rather than panicking.
#[test]
fn a_zero_page_count_is_rejected() {
    use crate::fs::scatter_gather_len;

    let error = scatter_gather_len(0).expect_err("zero pages must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

/// The length is checked before the buffer is allocated, so an unrepresentable
/// request costs nothing -- which is also what makes this test affordable: it
/// never allocates the 4GiB it asks for.
#[cfg(target_pointer_width = "64")]
#[test]
fn blocking_read_rejects_an_oversized_length() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-oversized-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"oversized read test").expect("create file");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
    );

    let error = endpoint
        .read(u32::MAX as usize + 1, 0)
        .expect_err("an unrepresentable length must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the request should be rejected before reaching the kernel: {error}"
    );

    let _ = std::fs::remove_file(&path);
}

/// A scatter-read reaches the same limit through a page count, and is likewise
/// rejected before any pages are allocated -- so a request that would need
/// terabytes of page buffers costs nothing.
#[cfg(target_pointer_width = "64")]
#[test]
fn blocking_read_scatter_rejects_an_oversized_page_count() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-oversized-scatter-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"oversized scatter test").expect("create file");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, FILE_FLAG_NO_BUFFERING)
            .expect("open endpoint"),
    );

    // Far more pages than u32::MAX bytes can describe.
    let error = endpoint
        .read_scatter(usize::MAX / PAGE_SIZE, 0)
        .expect_err("an unrepresentable page count must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the request should be rejected before reaching the kernel: {error}"
    );

    let _ = std::fs::remove_file(&path);
}

/// `read_scatter(0, ..)` must return `InvalidInput` rather than panicking in
/// `PageBuffers::new(0)`.
#[test]
fn blocking_read_scatter_rejects_a_zero_page_count() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-zero-scatter-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"zero scatter test").expect("create file");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, FILE_FLAG_NO_BUFFERING)
            .expect("open endpoint"),
    );

    let error = endpoint
        .read_scatter(0, 0)
        .expect_err("a zero page count must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        error.raw_os_error().is_none(),
        "the request should be rejected before reaching the kernel: {error}"
    );

    let _ = std::fs::remove_file(&path);
}
