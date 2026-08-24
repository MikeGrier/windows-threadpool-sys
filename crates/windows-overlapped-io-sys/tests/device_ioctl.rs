// Copyright (c) 2026 Mike Grier
//! Integration test (`device` feature): an `FSCTL` query on a real file through
//! both the blocking and IOCP `ioctl` adapters. `ioctl` is `unsafe` because it
//! takes an arbitrary control code; `FSCTL_GET_COMPRESSION` is self-contained,
//! which is what the `unsafe` blocks below assert.
//!
//! The skip-on-success tests below deliberately do not depend on the `fs`
//! feature (M13.2): `set_notification_modes` and the `device` adapters are both
//! core-and-`device`-only since M13.1, and a test proving that needed to be
//! buildable with `device` alone rather than nested inside a file gated on `fs`.

#![cfg(all(windows, feature = "device"))]

use std::path::PathBuf;

use windows_overlapped_io_sys::{
    BlockingEndpoint, CompletionPort, NotificationModes, Started, UnassociatedEndpoint,
};
use windows_sys::Win32::System::Ioctl::FSCTL_GET_COMPRESSION;

/// `FSCTL_GET_COMPRESSION` returns a `USHORT` compression state.
const COMPRESSION_STATE_LEN: usize = 2;

/// How long to wait for a packet that should not exist. Long enough that a
/// queued packet would have arrived, short enough not to stall the suite.
const NO_PACKET_TIMEOUT_MS: u32 = 250;

fn temp_file(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-ioctl-int-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"integration device control").expect("create temp file");
    path
}

fn skip_on_success() -> NotificationModes {
    NotificationModes {
        skip_completion_port_on_success: true,
        ..NotificationModes::default()
    }
}

#[test]
fn blocking_backend_queries_compression() {
    let path = temp_file("blocking");
    let mut endpoint = BlockingEndpoint::new(
        UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
    )
    .expect("no incompatible notification mode");

    // SAFETY: FSCTL_GET_COMPRESSION is self-contained -- empty input, and it
    // writes only the owned output buffer, embedding no pointers.
    let mut output = vec![0_u8; COMPRESSION_STATE_LEN];
    let returned =
        unsafe { endpoint.ioctl(FSCTL_GET_COMPRESSION, &[], &mut output) }.expect("ioctl");
    assert_eq!(returned, COMPRESSION_STATE_LEN);
    assert_eq!(output.len(), COMPRESSION_STATE_LEN);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn iocp_backend_queries_compression() {
    let path = temp_file("iocp");
    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    // SAFETY: FSCTL_GET_COMPRESSION is self-contained -- empty input, and it
    // writes only the owned output buffer, embedding no pointers.
    let token = unsafe {
        endpoint.ioctl(
            FSCTL_GET_COMPRESSION,
            Vec::new(),
            vec![0_u8; COMPRESSION_STATE_LEN],
        )
    }
    .expect("submit ioctl")
    .expect_pending("this endpoint is not in skip-on-success mode");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (output, result) = token
        .claim(&completion)
        .unwrap_or_else(|_| panic!("completion did not match its token"));
    let returned = result.expect("ioctl result");
    assert_eq!(returned, COMPRESSION_STATE_LEN);
    assert!(output.len() >= COMPRESSION_STATE_LEN);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

// --- skip-on-success (M13.2) ---

#[test]
fn an_ioctl_on_a_skip_endpoint_reports_whichever_arm_the_io_manager_chose() {
    let path = temp_file("skip");

    let mut endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    endpoint
        .set_notification_modes(skip_on_success())
        .expect("set skip-on-success");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port.associate(endpoint, 0).expect("associate");

    // SAFETY: FSCTL_GET_COMPRESSION is self-contained -- its input is empty
    // and it writes only the owned output buffer, embedding no pointers.
    let started = unsafe {
        endpoint.ioctl(
            FSCTL_GET_COMPRESSION,
            Vec::new(),
            vec![0_u8; COMPRESSION_STATE_LEN],
        )
    }
    .expect("submit ioctl");

    match started {
        Started::Completed {
            payload,
            bytes_transferred,
        } => {
            assert_eq!(bytes_transferred, COMPRESSION_STATE_LEN);
            assert!(payload.len() >= COMPRESSION_STATE_LEN);
            assert_eq!(port.outstanding(), 0);
            assert!(
                port.get(NO_PACKET_TIMEOUT_MS).expect("get").is_none(),
                "skip-on-success queued a packet for a synchronous success"
            );
        }
        Started::Pending(token) => {
            let completion = port.get(5_000).expect("get").expect("a completion");
            let (payload, result) = token.claim(&completion).expect("token matches");
            assert_eq!(result.expect("ioctl result"), COMPRESSION_STATE_LEN);
            assert!(payload.len() >= COMPRESSION_STATE_LEN);
            assert_eq!(port.outstanding(), 0);
        }
    }

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_same_ioctl_on_a_default_endpoint_is_always_pending() {
    // The contrast that gives the test above its meaning: without the mode, an
    // immediate success still queues a packet, so the adapter can only ever
    // report `Pending` -- this arm is exact, not tolerant.
    let path = temp_file("default");

    let port = CompletionPort::new(0).expect("create port");
    let endpoint = port
        .associate(
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint"),
            0,
        )
        .expect("associate");

    // SAFETY: as above.
    let started = unsafe {
        endpoint.ioctl(
            FSCTL_GET_COMPRESSION,
            Vec::new(),
            vec![0_u8; COMPRESSION_STATE_LEN],
        )
    }
    .expect("submit ioctl");
    assert!(
        started.is_pending(),
        "an endpoint in the default mode always gets a completion packet"
    );

    let token = started.expect_pending("just asserted pending");
    let completion = port.get(5_000).expect("get").expect("a completion");
    let (payload, result) = token.claim(&completion).expect("token matches");
    assert_eq!(result.expect("ioctl result"), COMPRESSION_STATE_LEN);
    assert!(payload.len() >= COMPRESSION_STATE_LEN);
    assert_eq!(port.outstanding(), 0);

    drop(endpoint);
    let _ = std::fs::remove_file(&path);
}

/// Requires the synchronous arm rather than tolerating either, the same
/// deliberate choice `socket_skip_on_success.rs` makes: a cached, self-contained
/// FSCTL on a freshly opened local file has every reason to complete inline, so
/// if this ever starts failing the message is "skip-on-success is no longer
/// being applied to this endpoint," not "flaky test." Several attempts are
/// made (against fresh endpoints, since a mode cannot be unset) so one
/// unlucky scheduling does not make the test flaky on its own.
#[test]
fn the_synchronous_arm_reports_the_full_byte_count() {
    let mut synchronous = 0_usize;
    for attempt in 0..5 {
        let path = temp_file(&format!("sync-count-{attempt}"));

        let mut endpoint =
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        endpoint
            .set_notification_modes(skip_on_success())
            .expect("set skip-on-success");

        let port = CompletionPort::new(0).expect("create port");
        let endpoint = port.associate(endpoint, 0).expect("associate");

        // SAFETY: as above.
        let started = unsafe {
            endpoint.ioctl(
                FSCTL_GET_COMPRESSION,
                Vec::new(),
                vec![0_u8; COMPRESSION_STATE_LEN],
            )
        }
        .expect("submit ioctl");

        match started {
            Started::Completed {
                bytes_transferred, ..
            } => {
                synchronous += 1;
                assert_eq!(
                    bytes_transferred, COMPRESSION_STATE_LEN,
                    "a synchronous ioctl reports the count the kernel wrote, not zero"
                );
                assert_eq!(port.outstanding(), 0);
            }
            Started::Pending(token) => {
                let completion = port.get(5_000).expect("get").expect("a completion");
                let (_payload, result) = token.claim(&completion).expect("token matches");
                assert_eq!(result.expect("ioctl result"), COMPRESSION_STATE_LEN);
            }
        }

        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    assert!(
        synchronous > 0,
        "no ioctl took the synchronous arm in 5 attempts, so skip-on-success \
         never applied and this file proved nothing about it"
    );
}
