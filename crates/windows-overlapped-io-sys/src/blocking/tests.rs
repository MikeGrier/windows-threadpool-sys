// Copyright (c) 2026 Mike Grier
use crate::{BlockingEndpoint, NotificationModes, UnassociatedEndpoint};

fn temp_file(tag: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-blocking-reject-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("create file");
    path
}

#[test]
fn rejects_an_endpoint_with_skip_set_event_on_handle() {
    let path = temp_file("skip-event");
    let mut endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    endpoint
        .set_notification_modes(NotificationModes {
            skip_set_event_on_handle: true,
            ..NotificationModes::default()
        })
        .expect("set notification modes");

    let error = BlockingEndpoint::new(endpoint).expect_err(
        "GetOverlappedResult's wait relies on exactly the notification this mode suppresses",
    );
    // The endpoint is recoverable, not lost, so a caller that constructed it
    // in error can still use it through a different backend.
    let _ = error.into_endpoint();
}

#[test]
fn accepts_an_endpoint_with_no_incompatible_mode() {
    let path = temp_file("plain");
    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    assert!(BlockingEndpoint::new(endpoint).is_ok());
}
