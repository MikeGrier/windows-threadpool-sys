// Copyright (c) 2026 Mike Grier
use super::UnassociatedEndpoint;
use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, OwnedHandle};

const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;

#[test]
fn borrows_and_reclaims_the_same_handle() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-{}.tmp",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&path)
        .expect("create overlapped temp file");
    let owned = OwnedHandle::from(file);
    let expected = owned.as_raw_handle();

    // SAFETY: the file was just created with FILE_FLAG_OVERLAPPED, is not
    // associated with any completion port, has no duplicates, and its
    // ownership moves into the endpoint.
    let endpoint = unsafe { UnassociatedEndpoint::assume_overlapped(owned) };
    assert_eq!(endpoint.handle().as_raw_handle(), expected);

    let recovered = endpoint.into_handle();
    assert_eq!(recovered.as_raw_handle(), expected);
    drop(recovered);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn open_creates_an_overlapped_endpoint() {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-open-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, b"x").expect("write temp file");

    let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
    // The safe creator yields a usable, borrowable handle.
    assert!(!endpoint.handle().as_raw_handle().is_null());
    drop(endpoint);

    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "fs")]
mod notification_modes {
    use super::super::{NotificationModes, UnassociatedEndpoint};
    use std::path::PathBuf;

    fn temp_endpoint(tag: &str) -> (UnassociatedEndpoint, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-modes-{tag}-{}.tmp",
            std::process::id()
        ));
        std::fs::write(&path, b"payload").expect("write temp file");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        (endpoint, path)
    }

    #[test]
    fn the_default_is_every_mode_off() {
        let modes = NotificationModes::default();
        assert!(!modes.skip_completion_port_on_success);
        assert!(!modes.skip_set_event_on_handle);
    }

    #[test]
    fn setting_no_modes_is_accepted_as_a_no_op() {
        let (endpoint, path) = temp_endpoint("none");
        endpoint
            .set_notification_modes(NotificationModes::default())
            .expect("setting no modes succeeds");
        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skip_on_success_is_accepted_on_a_file_endpoint() {
        let (endpoint, path) = temp_endpoint("skip");
        endpoint
            .set_notification_modes(NotificationModes {
                skip_completion_port_on_success: true,
                ..NotificationModes::default()
            })
            .expect("a file handle supports skip-on-success");
        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skip_set_event_on_handle_is_accepted_on_a_file_endpoint() {
        let (endpoint, path) = temp_endpoint("event");
        endpoint
            .set_notification_modes(NotificationModes {
                skip_set_event_on_handle: true,
                ..NotificationModes::default()
            })
            .expect("a file handle supports skip-set-event");
        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn both_modes_can_be_set_in_one_call() {
        let (endpoint, path) = temp_endpoint("both");
        endpoint
            .set_notification_modes(NotificationModes {
                skip_completion_port_on_success: true,
                skip_set_event_on_handle: true,
            })
            .expect("both modes set together");
        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn setting_modes_is_additive_across_calls() {
        // A mode cannot be removed once set, so a second call can only add to
        // the first; neither call is rejected for repeating what is already on.
        let (endpoint, path) = temp_endpoint("additive");
        let skip_success = NotificationModes {
            skip_completion_port_on_success: true,
            ..NotificationModes::default()
        };
        endpoint
            .set_notification_modes(skip_success)
            .expect("first call");
        endpoint
            .set_notification_modes(skip_success)
            .expect("repeating a mode already set");
        endpoint
            .set_notification_modes(NotificationModes {
                skip_set_event_on_handle: true,
                ..NotificationModes::default()
            })
            .expect("adding a second mode");
        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn modes_survive_being_carried_into_an_association() {
        // The flag is inert until the handle is associated with a port, so
        // setting it first (this type's whole point) must not be rejected and
        // must not interfere with the association itself.
        use crate::CompletionPort;

        let (endpoint, path) = temp_endpoint("associate");
        endpoint
            .set_notification_modes(NotificationModes {
                skip_completion_port_on_success: true,
                ..NotificationModes::default()
            })
            .expect("set before association");

        let port = CompletionPort::new(0).expect("create port");
        let associated = port.associate(endpoint, 0x99).expect("associate");
        assert_eq!(associated.key(), 0x99);

        drop(associated);
        drop(port);
        let _ = std::fs::remove_file(&path);
    }
}
