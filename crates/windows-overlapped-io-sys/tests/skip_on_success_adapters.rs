// Copyright (c) 2026 Mike Grier
//! Integration test: `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` end to end through
//! the buffer-owning adapters.
//!
//! The core seam has always handled skip-on-success; what these tests cover is
//! that the adapters now *report* it (`Started::Completed`) instead of
//! rejecting it, that the payload and byte count on that path match what the
//! pending path's `claim` would have produced, and that a synchronous
//! completion leaves nothing outstanding on the port.
//!
//! # Why each test tolerates both arms
//!
//! Whether a given request completes synchronously is the I/O Manager's call,
//! not something a caller can compel: a cached read usually does and an
//! uncached one usually does not, but neither is contractual. So a test that
//! *required* `Completed` would be flaky by construction. Each test instead
//! asserts the invariants that must hold for whichever arm it observes, which
//! is the honest form -- and the non-skip endpoints, where `Pending` *is*
//! guaranteed, are asserted exactly.

#![cfg(all(windows, feature = "fs"))]

use std::path::PathBuf;

use windows_overlapped_io_sys::{CompletionPort, NotificationModes, Started, UnassociatedEndpoint};

/// How long to wait for a packet that should not exist. Long enough that a
/// queued packet would have arrived, short enough not to stall the suite.
const NO_PACKET_TIMEOUT_MS: u32 = 250;

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-skip-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

fn skip_on_success() -> NotificationModes {
    NotificationModes {
        skip_completion_port_on_success: true,
        ..NotificationModes::default()
    }
}

#[test]
fn a_read_on_a_skip_endpoint_reports_whichever_arm_the_io_manager_chose() {
    let content = b"skip-on-success adapter read";
    let path = temp_file_with(content, "read");

    let mut endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    endpoint
        .set_notification_modes(skip_on_success())
        .expect("set skip-on-success");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port.associate(endpoint, 0).expect("associate");

    match endpoint
        .read(vec![0_u8; content.len()], 0)
        .expect("submit read")
    {
        Started::Completed {
            payload,
            bytes_transferred,
        } => {
            // The whole point of the mode: the result is already here.
            assert_eq!(bytes_transferred, content.len());
            assert_eq!(&payload[..bytes_transferred], content);
            // Reclaimed inline, so the port never counted it as outstanding
            // past the call -- rundown must not be waiting on anything.
            assert_eq!(port.outstanding(), 0);
            // And no packet was queued, which is what the mode buys.
            assert!(
                port.get(NO_PACKET_TIMEOUT_MS).expect("get").is_none(),
                "skip-on-success queued a packet for a synchronous success"
            );
        }
        Started::Pending(token) => {
            // The read went asynchronous, so the mode does not apply and the
            // ordinary path must still work exactly as before.
            let completion = port.get(5_000).expect("get").expect("a completion");
            let (payload, result) = token.claim(&completion).expect("token matches");
            let read = result.expect("read result");
            assert_eq!(read, content.len());
            assert_eq!(&payload[..read], content);
            assert_eq!(port.outstanding(), 0);
        }
    }

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_same_read_on_a_default_endpoint_is_always_pending() {
    // The contrast that gives the test above its meaning: without the mode, an
    // immediate success still queues a packet, so the adapter can only ever
    // report `Pending` -- this arm is exact, not tolerant.
    let content = b"default endpoint adapter read";
    let path = temp_file_with(content, "default");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    let started = endpoint
        .read(vec![0_u8; content.len()], 0)
        .expect("submit read");
    assert!(
        started.is_pending(),
        "an endpoint in the default mode always gets a completion packet"
    );

    let token = started.expect_pending("just asserted pending");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (payload, result) = token.claim(&completion).expect("token matches");
    let read = result.expect("read result");
    assert_eq!(read, content.len());
    assert_eq!(&payload[..read], content);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_write_on_a_skip_endpoint_returns_its_buffer_on_either_arm() {
    // The write path hands the caller's buffer back either way, so a caller
    // reusing a buffer pool gets it returned regardless of which arm ran.
    let path = temp_file_with(b"", "write");
    let data = b"skip-on-success adapter write".to_vec();
    let expected = data.clone();

    let mut endpoint = UnassociatedEndpoint::open(&path, true, true, 0).expect("open endpoint");
    endpoint
        .set_notification_modes(skip_on_success())
        .expect("set skip-on-success");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port.associate(endpoint, 0).expect("associate");

    let (payload, written) = match endpoint.write(data, 0).expect("submit write") {
        Started::Completed {
            payload,
            bytes_transferred,
        } => {
            assert_eq!(port.outstanding(), 0);
            assert!(
                port.get(NO_PACKET_TIMEOUT_MS).expect("get").is_none(),
                "skip-on-success queued a packet for a synchronous success"
            );
            (payload, bytes_transferred)
        }
        Started::Pending(token) => {
            let completion = port.get(5_000).expect("get").expect("a completion");
            let (payload, result) = token.claim(&completion).expect("token matches");
            (payload, result.expect("write result"))
        }
    };

    assert_eq!(written, expected.len());
    assert_eq!(payload, expected, "the write buffer comes back either way");
    assert_eq!(std::fs::read(&path).expect("read back"), expected);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}
