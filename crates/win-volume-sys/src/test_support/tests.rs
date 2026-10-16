// Copyright (c) 2026 Mike Grier

use super::{drive_of, temp_volume_path};

#[test]
fn a_plain_drive_path_names_its_drive() {
    assert_eq!(drive_of(r"C:\Temp\"), Some("C:"));
    assert_eq!(drive_of(r"d:\a\b"), Some("d:"));
    assert_eq!(drive_of("Z:"), Some("Z:"));
}

#[test]
fn a_verbatim_or_device_drive_path_names_its_drive() {
    assert_eq!(drive_of(r"\\?\C:\Temp\"), Some("C:"));
    assert_eq!(drive_of(r"\\?\e:\a\b"), Some("e:"));
    assert_eq!(drive_of(r"\\.\C:\Temp"), Some("C:"));
}

#[test]
fn a_path_with_no_drive_names_none() {
    for path in [
        "",
        "C",
        r"\\server\share\x",
        r"\\?\UNC\server\share\x",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
        r"\\?\",
        r"relative\path",
        r"1:\digit",
        // A multi-byte first character is not a drive letter, and is not sliced in the middle.
        "\u{e9}:\\x",
    ] {
        assert_eq!(drive_of(path), None, "{path:?}");
    }
}

#[test]
fn the_temporary_directory_names_a_device_path() {
    let path = temp_volume_path();
    let drive = path.strip_prefix(r"\\.\").expect("a device path");
    assert_eq!(drive.len(), 2, "{path}");
    assert!(drive.ends_with(':'), "{path}");
}
