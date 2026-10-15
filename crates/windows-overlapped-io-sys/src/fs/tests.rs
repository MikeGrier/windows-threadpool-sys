// Copyright (c) 2026 Mike Grier
use crate::{
    BlockingEndpoint, CompletionPort, FILE_FLAG_NO_BUFFERING, PageBuffers, UnassociatedEndpoint,
    page_size,
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
    )
    .expect("no incompatible notification mode");

    let data = b"safe file adapter round trip";
    let written = endpoint.write(data, 0).expect("write");
    assert_eq!(written, data.len());

    let mut buffer = vec![0_u8; data.len()];
    let read = endpoint.read(&mut buffer, 0).expect("read");
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
        .read_scatter(PageBuffers::new(2), 0)
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
    )
    .expect("no incompatible notification mode");

    let mut src = PageBuffers::new(2);
    for (i, byte) in src.as_bytes_mut().iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }

    let written = endpoint.write_gather(&src, 0).expect("write_gather");
    assert_eq!(written, src.len());

    let mut dst = PageBuffers::new(2);
    let read = endpoint.read_scatter(&mut dst, 0).expect("read_scatter");
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

    let content = vec![0xAB_u8; page_size()];
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

    assert_eq!(result.expect("read result"), page_size());
    assert_eq!(returned.stable_ptr(), expected_ptr);
    assert_eq!(returned.stable_ptr().addr() % page_size(), 0);
    assert_eq!(returned.as_bytes(), &content[..]);

    drop(endpoint);
    drop(port);
    let _ = std::fs::remove_file(&path);
}

// --- page size and buffer alignment, asked rather than assumed (M16.5) ---

mod alignment {
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

    use super::super::buffer_alignment;
    use crate::{
        AlignmentInfo, BlockingEndpoint, CompletionPort, FILE_FLAG_NO_BUFFERING, PageBuffers,
        UnassociatedEndpoint, page_size,
    };

    fn mask(requirement_mask: u32) -> AlignmentInfo {
        AlignmentInfo { requirement_mask }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-fs-align-{tag}-{}.tmp",
            std::process::id()
        ))
    }

    fn patterned(mut buffers: PageBuffers) -> PageBuffers {
        for (i, byte) in buffers.as_bytes_mut().iter_mut().enumerate() {
            *byte = (i % 241) as u8;
        }
        buffers
    }

    #[test]
    fn the_page_size_is_the_operating_systems() {
        let mut info = SYSTEM_INFO::default();
        // SAFETY: a live local of the right type; the call only writes it.
        unsafe { GetSystemInfo(&raw mut info) };
        assert_eq!(page_size(), info.dwPageSize as usize);
        assert!(page_size().is_power_of_two());
        assert_eq!(page_size(), page_size(), "kept, not re-asked differently");
    }

    #[test]
    fn the_buffer_alignment_is_the_larger_of_the_page_and_the_device() {
        for (page, requirement, expected) in [
            (4096, 0, 4096),
            (4096, 1, 4096),
            (4096, 0x1FF, 4096),
            (4096, 0xFFF, 4096),
            (4096, 0x1FFF, 8192),
            (4096, 0x3FFF, 16_384),
            (4096, 0xF_FFFF, 1 << 20),
            (65_536, 0x1FFF, 65_536),
            (65_536, 0x1_FFFF, 131_072),
        ] {
            assert_eq!(
                buffer_alignment(page, mask(requirement)),
                Some(expected),
                "page {page}, mask {requirement:#x}"
            );
        }
    }

    #[test]
    fn a_mask_not_of_the_documented_form_rounds_up_to_one_that_clears_every_bit() {
        for (requirement, expected) in [(0x100_u32, 4096), (0x1000, 8192), (0x2001, 16_384)] {
            let align = buffer_alignment(4096, mask(requirement)).expect("fits");
            assert_eq!(align, expected, "mask {requirement:#x}");
            assert!(mask(requirement).is_aligned(align), "mask {requirement:#x}");
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_widest_mask_still_fits_a_64_bit_target() {
        assert_eq!(buffer_alignment(4096, mask(u32::MAX)), Some(1 << 32));
    }

    #[test]
    fn new_buffers_are_page_aligned() {
        for pages in [1, 2, 7] {
            let buffers = PageBuffers::new(pages);
            assert_eq!(buffers.alignment(), page_size());
            assert_eq!(buffers.as_bytes().as_ptr().addr() % page_size(), 0);
            assert_eq!(buffers.len(), pages * page_size());
        }
    }

    #[test]
    fn aligned_buffers_meet_the_alignment_they_were_asked_for() {
        for requirement in [0_u32, 1, 0x1FF, 0xFFF, 0x1FFF, 0x3FFF, 0xF_FFFF] {
            let info = mask(requirement);
            let buffers = PageBuffers::with_alignment(3, info).expect("with_alignment");
            let base = buffers.as_bytes().as_ptr().addr();

            assert!(buffers.alignment() >= page_size(), "mask {requirement:#x}");
            assert_eq!(
                Some(buffers.alignment()),
                buffer_alignment(page_size(), info),
                "mask {requirement:#x}"
            );
            assert_eq!(base % buffers.alignment(), 0, "mask {requirement:#x}");
            assert!(info.is_aligned(base), "mask {requirement:#x}");
            assert_eq!(
                buffers.len(),
                3 * page_size(),
                "the size is pages, not alignment"
            );
            assert!(buffers.as_bytes().iter().all(|&b| b == 0), "zeroed");
        }
    }

    #[test]
    fn every_segment_is_page_aligned_and_the_first_meets_the_device() {
        let info = mask(0x3FFF);
        let buffers = PageBuffers::with_alignment(5, info).expect("with_alignment");
        let segments = buffers.segment_array();

        assert_eq!(segments.len(), 6, "five pages and the terminator");
        // SAFETY: reading the Buffer member of elements this array was built with.
        let addresses: Vec<usize> = segments[..5]
            .iter()
            .map(|segment| unsafe { segment.Buffer }.addr())
            .collect();
        assert!(info.is_aligned(addresses[0]));
        for (i, address) in addresses.iter().enumerate() {
            assert_eq!(address % page_size(), 0, "segment {i}");
            assert_eq!(*address, addresses[0] + i * page_size(), "segment {i}");
        }
        // SAFETY: as above, the terminator.
        assert_eq!(unsafe { segments[5].Alignment }, 0);
    }

    #[test]
    fn buffers_aligned_above_a_page_round_trip_through_blocking_scatter_gather() {
        let path = temp_path("blocking");
        std::fs::write(&path, b"").expect("create file");
        let mut endpoint = BlockingEndpoint::new(
            UnassociatedEndpoint::open(&path, true, true, FILE_FLAG_NO_BUFFERING)
                .expect("open endpoint"),
        )
        .expect("blocking endpoint");

        let above_a_page = mask(0x3FFF);
        let source = patterned(PageBuffers::with_alignment(4, above_a_page).expect("source"));
        let written = endpoint.write_gather(&source, 0).expect("write_gather");
        assert_eq!(written, source.len());

        let mut destination = PageBuffers::with_alignment(4, above_a_page).expect("destination");
        let read = endpoint
            .read_scatter(&mut destination, 0)
            .expect("read_scatter");
        assert_eq!(read, source.len());
        assert_eq!(destination.as_bytes(), source.as_bytes());

        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn buffers_aligned_as_the_endpoint_asks_round_trip_through_iocp_scatter_gather() {
        let path = temp_path("iocp");
        std::fs::write(&path, b"").expect("create file");
        let port = CompletionPort::new(0).expect("create port");
        let endpoint = UnassociatedEndpoint::open(&path, true, true, FILE_FLAG_NO_BUFFERING)
            .expect("open endpoint");
        let asked = endpoint
            .observer()
            .alignment_info()
            .expect("alignment_info");
        let endpoint = port.associate(endpoint, 0).expect("associate");

        let source = patterned(PageBuffers::with_alignment(2, asked).expect("source"));
        let expected = source.as_bytes().to_vec();
        let token = endpoint
            .write_gather(source, 0)
            .expect("submit write_gather")
            .expect_pending("not in skip-on-success mode");
        let completion = port.get(5_000).expect("get").expect("write completion");
        let (returned, result) = token.claim(&completion).expect("token matches");
        assert_eq!(result.expect("write result"), returned.len());

        let destination = PageBuffers::with_alignment(2, asked).expect("destination");
        let token = endpoint
            .read_scatter(destination, 0)
            .expect("submit read_scatter")
            .expect_pending("not in skip-on-success mode");
        let completion = port.get(5_000).expect("get").expect("read completion");
        let (buffers, result) = token.claim(&completion).expect("token matches");
        assert_eq!(result.expect("read result"), buffers.len());
        assert_eq!(buffers.as_bytes(), expected.as_slice());
        assert_eq!(port.outstanding(), 0);

        drop(endpoint);
        drop(port);
        let _ = std::fs::remove_file(&path);
    }
}
