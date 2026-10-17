// Copyright (c) 2026 Mike Grier

use std::io;

use win_volume_sys::DiskExtent;

use super::{
    OpenedVolume, VolumeOptions, VolumePath, VolumePathError, disk_flush_domain, domains_of,
};
use crate::dioring::tests::TempFile;
use crate::windows_backend::{Cached, FileOptions, OpenedFile, Unbuffered, Win32FileTarget};
use durable_ioring::{Capabilities, FlushDomain, Geometry, IoTarget, WarrantyClass};

fn extent(disk_number: u32) -> DiskExtent {
    DiskExtent {
        disk_number,
        starting_offset: 0,
        length: 1 << 20,
    }
}

/// A volume made of the parts a test gives it, over an ordinary file's handle: what is derived from
/// a volume does not depend on the handle being one, and opening one needs an administrator.
fn assembled<B: crate::windows_backend::Buffering>(
    extents: Vec<DiskExtent>,
    sector: u32,
    locked: bool,
) -> (OpenedVolume<B>, TempFile) {
    let temp = TempFile::new("ov-assembled");
    let file = OpenedFile::<Cached>::open(&temp.0, FileOptions::new().read().write())
        .expect("open a file");
    (
        OpenedVolume::assembled(file.handle().clone(), extents, sector, locked),
        temp,
    )
}

#[test]
fn a_disk_is_named_as_it_answers_and_two_targets_share_a_domain_by_equal_bytes() {
    assert_eq!(
        disk_flush_domain(0),
        FlushDomain::new(br"\\.\PhysicalDrive0".to_vec())
    );
    assert_eq!(
        disk_flush_domain(12),
        FlushDomain::new(br"\\.\PhysicalDrive12".to_vec())
    );
    assert_eq!(disk_flush_domain(3), disk_flush_domain(3));
    assert_ne!(disk_flush_domain(3), disk_flush_domain(30));
    assert_ne!(disk_flush_domain(1), disk_flush_domain(2));
}

#[test]
fn a_drive_letter_names_its_volume_in_either_case() {
    for (letter, path) in [
        ('C', r"\\.\C:"),
        ('c', r"\\.\C:"),
        ('z', r"\\.\Z:"),
        ('A', r"\\.\A:"),
    ] {
        assert_eq!(VolumePath::drive(letter).expect("a letter").as_str(), path);
    }
}

#[test]
fn anything_else_is_not_a_drive_letter() {
    for bad in ['1', ' ', ':', '\\', '\u{e9}', '\0', '-'] {
        assert_eq!(
            VolumePath::drive(bad),
            Err(VolumePathError(bad.to_string())),
            "{bad:?}"
        );
    }
}

const GUID: &str = "01234567-89ab-cdef-0123-456789abcdef";

#[test]
fn a_volume_guid_path_is_accepted_with_or_without_the_trailing_backslash() {
    let bare = format!(r"\\?\Volume{{{GUID}}}");
    for given in [bare.clone(), format!(r"{bare}\")] {
        assert_eq!(VolumePath::guid(&given).expect("valid").as_str(), bare);
    }
    let upper = format!(r"\\?\Volume{{{}}}", GUID.to_uppercase());
    assert!(
        VolumePath::guid(&upper).is_ok(),
        "hexadecimal in either case"
    );
}

#[test]
fn a_path_that_is_not_a_volume_guid_is_refused() {
    let wrong_lengths = r"\\?\Volume{0123456-89ab-cdef-0123-456789abcdef}";
    let not_hex = r"\\?\Volume{0123456g-89ab-cdef-0123-456789abcdef}";
    let four_groups = r"\\?\Volume{01234567-89ab-cdef-456789abcdef}";
    let six_groups = r"\\?\Volume{01234567-89ab-cdef-0123-4567-89abcdef}";
    for bad in [
        "",
        r"\\.\C:",
        r"C:\",
        r"\\?\Volume{}",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef",
        r"\\?\volume{01234567-89ab-cdef-0123-456789abcdef}",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\file",
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\\",
        r"\\server\share",
        wrong_lengths,
        not_hex,
        four_groups,
        six_groups,
    ] {
        assert_eq!(
            VolumePath::guid(bad),
            Err(VolumePathError(bad.to_owned())),
            "{bad:?}"
        );
    }
}

#[test]
fn a_refused_path_says_what_was_given() {
    let error = VolumePath::guid(r"\\server\share").expect_err("a share");
    assert!(error.to_string().contains(r"\\\\server\\share"), "{error}");
}

#[test]
fn a_volume_has_one_flush_domain_for_each_disk_it_sits_on_once_each_in_disk_order() {
    assert_eq!(domains_of(&[]), []);
    assert_eq!(domains_of(&[extent(4)]), [disk_flush_domain(4)]);
    assert_eq!(
        domains_of(&[extent(3), extent(1), extent(3), extent(1), extent(7)]),
        [
            disk_flush_domain(1),
            disk_flush_domain(3),
            disk_flush_domain(7)
        ]
    );
}

#[test]
fn an_unbuffered_volume_needs_its_sector_in_offset_length_and_buffer() {
    let (volume, _temp) = assembled::<Unbuffered>(vec![extent(2)], 4096, false);
    assert_eq!(volume.geometry(), Geometry::uniform(4096).expect("valid"));
    let (small, _temp) = assembled::<Unbuffered>(vec![extent(2)], 512, false);
    assert_eq!(small.geometry(), Geometry::uniform(512).expect("valid"));
}

#[test]
fn a_cached_volume_needs_its_sector_in_offset_and_length_only() {
    let (volume, _temp) = assembled::<Cached>(vec![extent(2)], 512, false);
    assert_eq!(
        volume.geometry(),
        Geometry::new(512, 512, 1).expect("valid")
    );
}

#[test]
fn a_volume_reports_what_it_was_opened_with() {
    let (volume, _temp) = assembled::<Unbuffered>(vec![extent(1), extent(2)], 512, true);
    assert!(volume.is_locked());
    assert_eq!(volume.disk_extents(), [extent(1), extent(2)]);
    assert_eq!(
        volume.flush_domains(),
        [disk_flush_domain(1), disk_flush_domain(2)]
    );
    let (loose, _temp) = assembled::<Unbuffered>(vec![extent(1)], 512, false);
    assert!(!loose.is_locked());
}

#[test]
fn an_unbuffered_volumes_target_claims_shared_device_and_reports_its_disks() {
    let (volume, _temp) = assembled::<Unbuffered>(vec![extent(5), extent(2)], 512, false);
    let target = Win32FileTarget::from_volume(volume);
    assert_eq!(target.warranty(), WarrantyClass::SharedDevice);
    assert_eq!(target.geometry(), Geometry::uniform(512).expect("valid"));
    assert_eq!(
        target.flush_domains(),
        [disk_flush_domain(2), disk_flush_domain(5)]
    );
    assert_eq!(
        target.capabilities(),
        Capabilities::NONE.with_write_through()
    );
}

#[test]
fn a_cached_volumes_target_claims_best_effort_and_no_write_through() {
    let (volume, _temp) = assembled::<Cached>(vec![extent(0)], 512, false);
    let target = Win32FileTarget::from_volume(volume);
    assert_eq!(target.warranty(), WarrantyClass::BestEffort);
    assert!(!target.capabilities().write_through());
    assert_eq!(target.flush_domains(), [disk_flush_domain(0)]);
}

#[test]
fn a_volumes_target_is_never_complete() {
    // However it was opened and whether or not it is locked: a lock keeps other handles off the
    // volume, not other writers off its disks.
    for locked in [false, true] {
        let (volume, _temp) = assembled::<Unbuffered>(vec![extent(0)], 512, locked);
        assert_ne!(
            Win32FileTarget::from_volume(volume).warranty(),
            WarrantyClass::Complete
        );
    }
}

#[test]
fn declared_domains_are_added_to_a_volumes_and_never_take_one_away() {
    let (volume, _temp) = assembled::<Unbuffered>(vec![extent(1), extent(2)], 512, false);
    let other = FlushDomain::new(b"a share".to_vec());
    let target = Win32FileTarget::from_volume(volume).with_domains([other.clone()]);
    assert_eq!(
        target.flush_domains(),
        [disk_flush_domain(1), disk_flush_domain(2), other.clone()]
    );

    // Declaring again replaces what was declared, and keeps what the volume found; declaring a
    // domain the volume found already adds nothing.
    let again = target.with_domains([disk_flush_domain(2), FlushDomain::new(b"another".to_vec())]);
    assert_eq!(
        again.flush_domains(),
        [
            disk_flush_domain(1),
            disk_flush_domain(2),
            FlushDomain::new(b"another".to_vec())
        ]
    );
    let cleared = again.with_domains([]);
    assert_eq!(
        cleared.flush_domains(),
        [disk_flush_domain(1), disk_flush_domain(2)],
        "none declared leaves what the disks gave"
    );
}

#[test]
fn a_file_targets_domains_are_only_what_is_declared() {
    let temp = TempFile::new("ov-file-domains");
    let file =
        OpenedFile::<Cached>::open(&temp.0, FileOptions::new().read().write()).expect("open");
    let target = Win32FileTarget::new(file).with_domains([disk_flush_domain(9)]);
    assert_eq!(target.flush_domains(), [disk_flush_domain(9)]);
}

#[test]
fn opening_a_volume_needs_an_administrator_and_otherwise_works_or_is_refused_for_that() {
    // Run elevated or not: this opens the system volume for writing and does nothing with it, so
    // the one thing it asserts is that the open either succeeds and answers its controls, or is
    // refused for want of privilege, and never fails in some other way.
    let system = std::env::var("SystemDrive")
        .ok()
        .and_then(|drive| drive.chars().next())
        .unwrap_or('C');
    let path = VolumePath::drive(system).expect("a drive letter");
    match OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new()) {
        Ok(volume) => {
            assert!(!volume.is_locked());
            assert!(!volume.disk_extents().is_empty());
            assert!(volume.geometry().offset_alignment().is_power_of_two());
        }
        Err(error) => assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}"),
    }
}

#[test]
fn a_volume_that_is_not_there_is_refused() {
    let free = ('D'..='Z')
        .rev()
        .find(|letter| !std::path::Path::new(&format!("{letter}:\\")).exists())
        .expect("a free letter");
    let path = VolumePath::drive(free).expect("a letter");
    assert!(OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new()).is_err());
}
