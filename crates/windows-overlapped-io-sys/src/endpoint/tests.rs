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

    let recovered = endpoint
        .into_handle()
        .expect("no observer holds the handle");
    assert_eq!(recovered.as_raw_handle(), expected);
    drop(recovered);

    let _ = std::fs::remove_file(&path);
}

mod observers {
    use super::super::{NotificationModes, UnassociatedEndpoint};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    fn temp_file(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-observer-{tag}-{}.tmp",
            std::process::id()
        ));
        std::fs::write(&path, b"x").expect("write temp file");
        path
    }

    /// Whether the file can be opened with no sharing, which fails while any handle to it is open.
    fn no_handle_is_open(path: &Path) -> bool {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(path)
            .is_ok()
    }

    #[test]
    fn an_observer_keeps_the_handle_open_after_the_endpoint_drops() {
        let path = temp_file("outlives");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let observer = endpoint.observer();
        assert!(!no_handle_is_open(&path));

        drop(endpoint);
        assert!(
            !no_handle_is_open(&path),
            "the observer must keep the handle open"
        );

        drop(observer);
        assert!(
            no_handle_is_open(&path),
            "the last holder must close the handle"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn observers_share_the_endpoints_handle() {
        let path = temp_file("same");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let first = endpoint.observer();
        let second = endpoint.observer();
        let clone = first.clone();

        assert!(first.same_handle(&second));
        assert!(first.same_handle(&clone));
        assert_eq!(first.raw_for_test(), endpoint.handle().as_raw_handle());

        let other_path = temp_file("other");
        let other = UnassociatedEndpoint::open(&other_path, true, false, 0).expect("open other");
        assert!(!first.same_handle(&other.observer()));

        drop((endpoint, first, second, clone, other));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&other_path);
    }

    #[test]
    fn into_handle_is_refused_while_an_observer_lives_and_keeps_the_endpoint_intact() {
        let path = temp_file("refused");
        let mut endpoint =
            UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let modes = NotificationModes {
            skip_completion_port_on_success: true,
            skip_set_event_on_handle: false,
        };
        endpoint.set_notification_modes(modes).expect("set modes");
        let expected = endpoint.handle().as_raw_handle();
        let observer = endpoint.observer();

        let endpoint = endpoint
            .into_handle()
            .expect_err("an observer still holds the handle");
        assert_eq!(endpoint.handle().as_raw_handle(), expected);
        assert_eq!(endpoint.notification_modes(), modes);

        drop(observer);
        let owned = endpoint.into_handle().expect("the observer has dropped");
        assert_eq!(owned.as_raw_handle(), expected);
        drop(owned);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_endpoint_handle_hands_out_observers_of_the_same_handle() {
        let path = temp_file("endpoint-handle");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let before = endpoint.observer();
        let held = endpoint.into_endpoint_handle();
        let after = held.observer();
        assert!(before.same_handle(&after));
        assert_eq!(after.raw_for_test(), held.as_raw_handle());

        drop(held);
        assert!(!no_handle_is_open(&path));
        drop((before, after));
        assert!(no_handle_is_open(&path));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_observer_outlives_a_blocking_endpoint() {
        let path = temp_file("blocking");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let blocking = crate::BlockingEndpoint::new(endpoint).expect("blocking endpoint");
        let observer = blocking.observer();
        assert_eq!(observer.raw_for_test(), blocking.handle().as_raw_handle());

        drop(blocking);
        assert!(!no_handle_is_open(&path));
        drop(observer);
        assert!(no_handle_is_open(&path));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_observer_outlives_an_associated_endpoint() {
        let path = temp_file("associated");
        let port = crate::CompletionPort::new(1).expect("create port");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let early = endpoint.observer();
        let associated = port.associate(endpoint, 0x15).expect("associate");
        let late = associated.observer();
        assert!(early.same_handle(&late));
        assert_eq!(late.raw_for_test(), associated.handle().as_raw_handle());

        drop(associated);
        assert!(!no_handle_is_open(&path));
        drop((early, late));
        assert!(no_handle_is_open(&path));
        drop(port);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(feature = "fs")]
mod observer_queries {
    use super::super::UnassociatedEndpoint;
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "windows-overlapped-io-sys-query-{tag}-{}.tmp",
            std::process::id()
        ))
    }

    /// `FSCTL_SET_SPARSE`, which `windows-sys` exports only behind `Win32_System_Ioctl`.
    const FSCTL_SET_SPARSE: u32 = 0x0009_00C4;

    /// Mark `file` sparse, so extending it past 4 GiB allocates nothing.
    fn make_sparse(file: &std::fs::File) {
        let mut returned = 0_u32;
        // SAFETY: a live handle, no input or output buffer, and a synchronous
        // (non-overlapped) handle, so nothing outlives the call.
        let ok = unsafe {
            windows_sys::Win32::System::IO::DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_SPARSE,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &raw mut returned,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(
            ok,
            0,
            "FSCTL_SET_SPARSE: {}",
            std::io::Error::last_os_error()
        );
    }

    #[test]
    fn file_len_reports_each_length_set_through_another_handle() {
        let path = temp_path("sizes");
        let writer = std::fs::File::create(&path).expect("create temp file");
        make_sparse(&writer);
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let observer = endpoint.observer();

        // Growing and shrinking, across page sizes and the 32-bit boundary.
        let sizes: [u64; 12] = [
            0,
            1,
            4095,
            4096,
            4097,
            65_536,
            1 << 20,
            (1 << 32) - 1,
            1 << 32,
            (5 << 30) + 3,
            17,
            0,
        ];
        for size in sizes {
            writer.set_len(size).expect("set length");
            assert_eq!(observer.file_len().expect("file_len"), size, "size {size}");
        }

        drop((observer, endpoint, writer));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_len_follows_appends_as_they_land() {
        let path = temp_path("appends");
        let mut writer = std::fs::File::create(&path).expect("create temp file");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let observer = endpoint.observer();

        let mut expected = 0_u64;
        for chunk in [1_usize, 7, 512, 4096, 3] {
            writer.write_all(&vec![b'a'; chunk]).expect("append");
            expected += chunk as u64;
            assert_eq!(observer.file_len().expect("file_len"), expected);
        }

        drop((observer, endpoint, writer));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_observer_of_a_handle_reads_the_same_length() {
        let path = temp_path("agree");
        std::fs::write(&path, b"twelve bytes").expect("write temp file");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let first = endpoint.observer();
        let clone = first.clone();
        let held = endpoint.into_endpoint_handle();
        let late = held.observer();

        for observer in [&first, &clone, &late] {
            assert_eq!(observer.file_len().expect("file_len"), 12);
        }

        drop((first, clone, late, held));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_len_needs_no_read_access() {
        let path = temp_path("no-read");
        std::fs::write(&path, b"abc").expect("write temp file");
        let endpoint = UnassociatedEndpoint::open(&path, false, true, 0).expect("open endpoint");
        assert_eq!(endpoint.observer().file_len().expect("file_len"), 3);

        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_unsafe_borrow_lends_the_endpoints_own_handle() {
        let path = temp_path("borrow");
        std::fs::write(&path, b"x").expect("write temp file");
        let endpoint = UnassociatedEndpoint::open(&path, true, false, 0).expect("open endpoint");
        let observer = endpoint.observer();

        // SAFETY: the borrow is only compared, never used for I/O.
        let borrowed = unsafe { observer.handle() };
        assert_eq!(borrowed.as_raw_handle(), endpoint.handle().as_raw_handle());

        drop((observer, endpoint));
        let _ = std::fs::remove_file(&path);
    }
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
        let (mut endpoint, path) = temp_endpoint("none");
        endpoint
            .set_notification_modes(NotificationModes::default())
            .expect("setting no modes succeeds");
        drop(endpoint);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn skip_on_success_is_accepted_on_a_file_endpoint() {
        let (mut endpoint, path) = temp_endpoint("skip");
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
        let (mut endpoint, path) = temp_endpoint("event");
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
        let (mut endpoint, path) = temp_endpoint("both");
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
        let (mut endpoint, path) = temp_endpoint("additive");
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

        let (mut endpoint, path) = temp_endpoint("associate");
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
