// Copyright (c) 2026 Mike Grier

use std::io;
use std::mem::{offset_of, size_of};

use windows_sys::Win32::System::Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS};

use windows_sys::Win32::Foundation::ERROR_MORE_DATA;

use super::{ATTEMPTS, DiskExtent, extents_via, parse, volume_disk_extents};
use crate::test_support::{open_volume, ordinary_file, system_volume_path, temp_volume_path};

/// The answer's layout as Windows fixes it, written out here and not computed from the offsets
/// the parser uses, so a parser that read the wrong ones would not agree with it: a header of a
/// count and four bytes of padding, then each extent a disk number, four of padding, an offset and
/// a length.
const HEADER: usize = 8;
const EXTENT: usize = 24;

fn answer(extents: &[(u32, i64, i64)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(extents.len() as u32).to_ne_bytes());
    bytes.extend_from_slice(&[0; 4]);
    for &(disk, start, length) in extents {
        bytes.extend_from_slice(&disk.to_ne_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&start.to_ne_bytes());
        bytes.extend_from_slice(&length.to_ne_bytes());
    }
    bytes
}

fn invalid(result: io::Result<Vec<DiskExtent>>) -> bool {
    matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidData)
}

#[test]
fn the_layout_this_crate_reads_is_the_one_windows_defines() {
    assert_eq!(offset_of!(VOLUME_DISK_EXTENTS, Extents), HEADER);
    assert_eq!(size_of::<DISK_EXTENT>(), EXTENT);
    assert_eq!(offset_of!(DISK_EXTENT, StartingOffset), 8);
    assert_eq!(offset_of!(DISK_EXTENT, ExtentLength), 16);
}

#[test]
fn one_extent_is_read() {
    let parsed = parse(&answer(&[(2, 1_048_576, 4_194_304)])).expect("parse");
    assert_eq!(
        parsed,
        [DiskExtent {
            disk_number: 2,
            starting_offset: 1_048_576,
            length: 4_194_304,
        }]
    );
}

#[test]
fn several_extents_keep_the_order_the_device_gave() {
    let parsed = parse(&answer(&[(3, 0, 100), (1, 512, 200), (3, 4096, 300)])).expect("parse");
    let disks: Vec<u32> = parsed.iter().map(|extent| extent.disk_number).collect();
    assert_eq!(disks, [3, 1, 3]);
    assert_eq!(parsed[1].starting_offset, 512);
    assert_eq!(parsed[2].length, 300);
}

#[test]
fn no_extents_is_an_answer_and_not_an_error() {
    assert_eq!(parse(&answer(&[])).expect("parse"), []);
}

#[test]
fn an_answer_that_cannot_hold_its_count_is_refused() {
    assert!(invalid(parse(&[])), "empty");
    assert!(invalid(parse(&[1, 0, 0])), "shorter than a count");
    let mut one = answer(&[(0, 0, 1)]);
    one[0] = 2;
    assert!(invalid(parse(&one)), "a count of two over one extent");
    let mut cut = answer(&[(0, 0, 1), (1, 1, 1)]);
    cut.truncate(HEADER + EXTENT + EXTENT / 2);
    assert!(invalid(parse(&cut)), "the second extent cut short");
}

#[test]
fn a_count_larger_than_any_buffer_is_refused_without_reading_or_allocating() {
    let mut bytes = answer(&[]);
    bytes[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(invalid(parse(&bytes)));
}

#[test]
fn a_negative_offset_or_length_is_refused() {
    assert!(invalid(parse(&answer(&[(0, -1, 10)]))), "offset");
    assert!(invalid(parse(&answer(&[(0, 0, -10)]))), "length");
    assert!(
        invalid(parse(&answer(&[(0, 0, 10), (1, i64::MIN, 10)]))),
        "the second"
    );
}

#[test]
fn bytes_after_the_counted_extents_are_ignored() {
    let mut padded = answer(&[(5, 0, 8)]);
    padded.extend_from_slice(&[0xFF; 64]);
    let parsed = parse(&padded).expect("parse");
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].disk_number, 5);
}

#[test]
fn the_largest_values_a_device_can_report_survive() {
    let parsed = parse(&answer(&[(u32::MAX, i64::MAX, i64::MAX)])).expect("parse");
    assert_eq!(parsed[0].disk_number, u32::MAX);
    assert_eq!(parsed[0].starting_offset, i64::MAX as u64);
    assert_eq!(parsed[0].length, i64::MAX as u64);
}

#[test]
fn the_system_volume_sits_on_at_least_one_disk() {
    let volume = open_volume(&system_volume_path(), false);
    let extents = volume_disk_extents(&volume).expect("the extents of the system volume");
    assert!(!extents.is_empty());
    assert!(
        extents.iter().all(|extent| extent.length > 0),
        "{extents:?}"
    );
}

#[test]
fn the_temporary_directorys_volume_answers_too() {
    let volume = open_volume(&temp_volume_path(), false);
    let extents = volume_disk_extents(&volume).expect("the extents");
    assert!(!extents.is_empty());
}

#[test]
fn an_overlapped_handle_is_waited_for_and_gives_the_same_answer() {
    let path = system_volume_path();
    let plain = volume_disk_extents(&open_volume(&path, false)).expect("plain");
    let overlapped = volume_disk_extents(&open_volume(&path, true)).expect("overlapped");
    assert_eq!(plain, overlapped);
}

/// A device that holds `extents` and answers as the real one does: `ERROR_MORE_DATA` with the count
/// in the header when the buffer cannot hold them all, the extents when it can.
fn device<'a>(
    extents: &'static [(u32, i64, i64)],
    asked: &'a mut Vec<usize>,
) -> impl FnMut(&mut [u8]) -> io::Result<usize> + 'a {
    move |buffer: &mut [u8]| {
        asked.push(buffer.len());
        let full = answer(extents);
        if buffer.len() < full.len() {
            buffer[..4].copy_from_slice(&(extents.len() as u32).to_ne_bytes());
            return Err(io::Error::from_raw_os_error(ERROR_MORE_DATA as i32));
        }
        buffer[..full.len()].copy_from_slice(&full);
        Ok(full.len())
    }
}

#[test]
fn an_answer_that_does_not_fit_is_asked_for_again_at_the_size_it_needs() {
    const HELD: &[(u32, i64, i64)] = &[(0, 0, 10), (1, 0, 20), (2, 0, 30), (3, 0, 40)];
    let mut asked = Vec::new();
    let parsed = extents_via(1, device(HELD, &mut asked)).expect("asked again");
    assert_eq!(parsed.len(), 4);
    assert_eq!(parsed[3].disk_number, 3);
    assert_eq!(asked, [HEADER + EXTENT, HEADER + 4 * EXTENT]);
}

#[test]
fn room_for_none_is_room_for_one_as_the_device_needs() {
    const ONE: &[(u32, i64, i64)] = &[(7, 0, 10)];
    let mut asked = Vec::new();
    extents_via(0, device(ONE, &mut asked)).expect("answered");
    assert_eq!(asked, [HEADER + EXTENT]);
}

#[test]
fn an_answer_that_keeps_growing_is_given_up_on() {
    let mut asked = 0usize;
    let error = extents_via(1, |buffer: &mut [u8]| {
        asked += 1;
        buffer[..4].copy_from_slice(&((asked as u32 + 1) * 10).to_ne_bytes());
        Err(io::Error::from_raw_os_error(ERROR_MORE_DATA as i32))
    })
    .expect_err("never settles");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(asked, ATTEMPTS);
}

#[test]
fn any_other_error_from_the_device_is_the_callers() {
    let error =
        extents_via(1, |_: &mut [u8]| Err(io::Error::from_raw_os_error(5))).expect_err("refused");
    assert_eq!(error.raw_os_error(), Some(5));
}

#[test]
fn a_handle_that_is_not_a_volume_is_refused_by_the_device() {
    let (file, path) = ordinary_file("not-a-volume");
    let refused = volume_disk_extents(&file).expect_err("a file has no disk extents");
    assert!(
        refused.raw_os_error().is_some(),
        "the device's own error: {refused}"
    );
    drop(file);
    std::fs::remove_file(path).expect("remove");
}
