// Copyright (c) 2026 Mike Grier

use std::io;

use super::{DiskPath, OpenedDisk};
use crate::dioring::tests::TempFile;
use crate::windows_backend::{
    Cached, FileOptions, FileRights, OpenedFile, Unbuffered, Win32FileTarget, disk_flush_domain,
};
use durable_ioring::{Capabilities, Geometry, IoTarget, WarrantyClass};

/// A disk made of the parts a test gives it, over an ordinary file's handle: what is derived from a
/// disk does not depend on the handle being one, and opening one needs an administrator.
fn assembled<B: crate::windows_backend::Buffering>(
    disk_number: u32,
    sector: u32,
) -> (OpenedDisk<B>, TempFile) {
    let temp = TempFile::new("od-assembled");
    let file = OpenedFile::<Cached>::open(
        &temp.0,
        FileOptions::new().access(FileRights::GENERIC_READ | FileRights::GENERIC_WRITE),
    )
    .expect("open a file");
    (
        OpenedDisk::assembled(file.handle().clone(), disk_number, sector),
        temp,
    )
}

#[test]
fn a_disk_is_named_by_its_number_in_the_one_form_that_names_a_disk() {
    for (number, path) in [
        (0, r"\\.\PhysicalDrive0"),
        (1, r"\\.\PhysicalDrive1"),
        (17, r"\\.\PhysicalDrive17"),
        (u32::MAX, r"\\.\PhysicalDrive4294967295"),
    ] {
        let disk = DiskPath::number(number);
        assert_eq!(disk.path(), path);
        assert_eq!(disk.to_string(), path);
        assert_eq!(disk.disk_number(), number);
    }
    assert_eq!(DiskPath::number(3), DiskPath::number(3));
    assert_ne!(DiskPath::number(3), DiskPath::number(30));
}

#[test]
fn a_disks_flush_domain_is_itself_in_the_bytes_a_volume_on_it_would_name() {
    let (disk, _temp) = assembled::<Unbuffered>(4, 512);
    assert_eq!(disk.disk_number(), 4);
    assert_eq!(disk.flush_domain(), disk_flush_domain(4));
}

#[test]
fn an_unbuffered_disk_needs_its_sector_in_offset_length_and_buffer() {
    let (disk, _temp) = assembled::<Unbuffered>(1, 4096);
    assert_eq!(disk.geometry(), Geometry::uniform(4096).expect("valid"));
}

#[test]
fn a_cached_disk_needs_its_sector_in_offset_and_length_only() {
    let (disk, _temp) = assembled::<Cached>(1, 512);
    assert_eq!(disk.geometry(), Geometry::new(512, 512, 1).expect("valid"));
}

#[test]
fn an_unbuffered_disks_target_claims_shared_device_and_reports_the_disk() {
    let (disk, _temp) = assembled::<Unbuffered>(6, 512);
    let target = Win32FileTarget::from_disk(disk);
    assert_eq!(target.warranty(), WarrantyClass::SharedDevice);
    assert_eq!(target.geometry(), Geometry::uniform(512).expect("valid"));
    assert_eq!(target.flush_domains(), [disk_flush_domain(6)]);
    assert_eq!(
        target.capabilities(),
        Capabilities::NONE.with_write_through()
    );
}

#[test]
fn a_cached_disks_target_claims_best_effort_and_no_write_through() {
    let (disk, _temp) = assembled::<Cached>(2, 512);
    let target = Win32FileTarget::from_disk(disk);
    assert_eq!(target.warranty(), WarrantyClass::BestEffort);
    assert!(!target.capabilities().write_through());
    assert_eq!(target.flush_domains(), [disk_flush_domain(2)]);
}

#[test]
fn a_disks_target_is_never_complete() {
    let (disk, _temp) = assembled::<Unbuffered>(0, 512);
    assert_ne!(
        Win32FileTarget::from_disk(disk).warranty(),
        WarrantyClass::Complete
    );
}

#[test]
fn declared_domains_are_added_to_the_disks_own_and_never_take_it_away() {
    let (disk, _temp) = assembled::<Unbuffered>(5, 512);
    let other = durable_ioring::FlushDomain::new(b"a share".to_vec());
    let target = Win32FileTarget::from_disk(disk).with_domains([other.clone()]);
    assert_eq!(target.flush_domains(), [disk_flush_domain(5), other]);
    assert_eq!(
        target.with_domains([]).flush_domains(),
        [disk_flush_domain(5)]
    );
}

#[test]
fn a_disk_number_nothing_has_is_refused() {
    // Not opened for any purpose but to be refused: no disk is numbered this, so the open fails
    // whatever the account, and never touches a real one.
    let error =
        OpenedDisk::<Unbuffered>::open(DiskPath::number(u32::MAX - 1)).expect_err("no such disk");
    assert!(
        matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
        ),
        "{error}"
    );
}
