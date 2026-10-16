// Copyright (c) 2026 Mike Grier

use std::io;
use std::mem::{offset_of, size_of};

use windows_sys::Win32::System::Ioctl::DISK_GEOMETRY;

use super::{bytes_per_sector, sector_size};
use crate::test_support::{open_volume, ordinary_file, system_volume_path, temp_volume_path};

/// A geometry answer with `size` as its sector size, the other fields zero.
fn geometry(size: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; size_of::<DISK_GEOMETRY>()];
    let at = offset_of!(DISK_GEOMETRY, BytesPerSector);
    bytes[at..at + 4].copy_from_slice(&size.to_ne_bytes());
    bytes
}

#[test]
fn the_sector_size_is_read_from_the_geometry() {
    for size in [512, 520, 4096, 65536, u32::MAX] {
        assert_eq!(sector_size(&geometry(size)).expect("a size"), size);
    }
}

#[test]
fn a_geometry_with_no_sector_size_or_too_short_to_hold_one_is_refused() {
    for answer in [geometry(0), Vec::new(), vec![0; 3]] {
        let error = sector_size(&answer).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{answer:?}");
    }
    let cut = &geometry(512)[..offset_of!(DISK_GEOMETRY, BytesPerSector) + 3];
    assert!(sector_size(cut).is_err(), "cut inside the field");
}

#[test]
fn a_real_volumes_sector_size_is_a_power_of_two_of_at_least_512() {
    for path in [system_volume_path(), temp_volume_path()] {
        let size = bytes_per_sector(&open_volume(&path, false)).expect("a sector size");
        assert!(size.is_power_of_two() && size >= 512, "{path}: {size}");
    }
}

#[test]
fn an_overlapped_handle_gives_the_same_size() {
    let path = system_volume_path();
    assert_eq!(
        bytes_per_sector(&open_volume(&path, false)).expect("plain"),
        bytes_per_sector(&open_volume(&path, true)).expect("overlapped")
    );
}

#[test]
fn a_handle_that_is_not_a_volume_is_refused_by_the_device() {
    let (file, path) = ordinary_file("geometry");
    assert!(bytes_per_sector(&file).is_err());
    drop(file);
    std::fs::remove_file(path).expect("remove");
}
