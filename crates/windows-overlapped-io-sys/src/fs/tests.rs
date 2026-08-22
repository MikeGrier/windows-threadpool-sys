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
        .read(vec![0_u8; content.len()], 0)
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

// --- caller-supplied owned buffers (M11) ---

/// Open a temp file holding `content` and associate it with a fresh port.
fn iocp_endpoint<'port>(
    port: &'port CompletionPort,
    content: &[u8],
    tag: &str,
) -> (crate::AssociatedEndpoint<'port>, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-fs-buf-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write file");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, true, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");
    (endpoint, path)
}

#[test]
fn a_written_buffer_comes_back_as_the_same_allocation() {
    // The point of the whole owned-buffer design: the adapter must not copy the
    // caller's bytes, so what `claim` returns has to be the very allocation that
    // was handed in, not an equal one.
    use crate::IoBuf;

    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = iocp_endpoint(&port, b"", "same-alloc");

    let buffer = b"no copies anywhere in this path".to_vec();
    let expected_len = buffer.len();
    let expected_ptr = buffer.stable_ptr();

    let token = endpoint
        .write(buffer, 0)
        .expect("submit write")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (returned, result) = token.claim(&completion).expect("token matches");

    assert_eq!(result.expect("write result"), expected_len);
    assert_eq!(
        returned.stable_ptr(),
        expected_ptr,
        "the buffer was copied somewhere along the way"
    );

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_read_fills_the_callers_own_buffer_in_place() {
    use crate::IoBuf;

    let content = b"read straight into the caller's pages";
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = iocp_endpoint(&port, content, "read-in-place");

    let buffer = vec![0_u8; content.len()];
    let expected_ptr = buffer.stable_ptr();

    let token = endpoint
        .read(buffer, 0)
        .expect("submit read")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (returned, result) = token.claim(&completion).expect("token matches");

    let read = result.expect("read result");
    assert_eq!(read, content.len());
    assert_eq!(&returned[..read], content);
    assert_eq!(
        returned.stable_ptr(),
        expected_ptr,
        "the read did not land in the caller's own buffer"
    );

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_shared_arc_buffer_can_be_written_without_copying_it() {
    // The case that motivated splitting `IoBuf` from `IoBufMut`: an `Arc<[u8]>`
    // is a legitimate source, and sending it must not deep-copy the payload just
    // to satisfy the adapter.
    use crate::IoBuf;
    use std::sync::Arc;

    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = iocp_endpoint(&port, b"", "arc");

    let shared: Arc<[u8]> = Arc::from(b"shared bytes sent without a copy".to_vec());
    let expected_ptr = shared.stable_ptr();
    let expected_len = shared.len();
    // A second owner, proving the bytes really are shared while in flight.
    let observer = Arc::clone(&shared);

    let token = endpoint
        .write(shared, 0)
        .expect("submit write")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (returned, result) = token.claim(&completion).expect("token matches");

    assert_eq!(result.expect("write result"), expected_len);
    assert_eq!(returned.stable_ptr(), expected_ptr);
    assert_eq!(
        observer.stable_ptr(),
        expected_ptr,
        "the other owner must still name the same bytes"
    );
    assert_eq!(std::fs::read(&path).expect("read back"), &*observer);

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_boxed_slice_round_trips_through_a_write() {
    use crate::IoBuf;

    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = iocp_endpoint(&port, b"", "boxed");

    let buffer: Box<[u8]> = b"a boxed slice needs no conversion"
        .to_vec()
        .into_boxed_slice();
    let expected_ptr = buffer.stable_ptr();
    let expected_len = buffer.len();

    let token = endpoint
        .write(buffer, 0)
        .expect("submit write")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (returned, result) = token.claim(&completion).expect("token matches");

    assert_eq!(result.expect("write result"), expected_len);
    assert_eq!(returned.stable_ptr(), expected_ptr);

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_static_slice_can_be_written_with_no_allocation_at_all() {
    use crate::IoBuf;

    const PAYLOAD: &[u8] = b"a static payload owns nothing";

    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = iocp_endpoint(&port, b"", "static");

    let token = endpoint
        .write(PAYLOAD, 0)
        .expect("submit write")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (returned, result) = token.claim(&completion).expect("token matches");

    assert_eq!(result.expect("write result"), PAYLOAD.len());
    assert_eq!(returned.stable_ptr(), PAYLOAD.as_ptr());

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn page_buffers_can_be_read_into_directly_keeping_their_alignment() {
    // A caller that chose `PageBuffers` for alignment must be able to hand it
    // straight to an ordinary read, rather than converting through a `Vec` and
    // losing both the alignment and a copy.
    use crate::IoBuf;

    let content = vec![0xAB_u8; PAGE_SIZE];
    let port = CompletionPort::new(0).expect("create port");
    let (endpoint, path) = iocp_endpoint(&port, &content, "pages");

    let buffers = PageBuffers::new(1);
    let expected_ptr = buffers.stable_ptr();

    let token = endpoint
        .read(buffers, 0)
        .expect("submit read")
        .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (returned, result) = token.claim(&completion).expect("token matches");

    assert_eq!(result.expect("read result"), PAGE_SIZE);
    assert_eq!(returned.stable_ptr(), expected_ptr);
    assert_eq!(returned.stable_ptr().addr() % PAGE_SIZE, 0);
    assert_eq!(returned.as_bytes(), &content[..]);

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}
