// Copyright (c) Mike Grier.

//! Tests for the `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` entry.
//!
//! A machine of one disk can show a real volume's single run and the device's refusals, and cannot
//! show a volume that spans disks, so the retry is tested against a fake device that answers as the
//! real one does: it writes the count even when the extents do not fit, and says `ERROR_MORE_DATA`.

use std::fs::{File, OpenOptions};
use std::mem::offset_of;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsHandle, AsRawHandle};

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, ERROR_MORE_DATA, ERROR_NOT_ENOUGH_MEMORY,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::Ioctl::{DISK_EXTENT, VOLUME_DISK_EXTENTS};

use super::{
    DiskExtent, DiskExtentsError, FIRST_EXTENTS, MAX_ATTEMPTS, QueryVolumeDiskExtents, answer_size,
    decode, extents_via, zeroed,
};
use crate::device_control::device_control;
use crate::handle::tests::{Fixture, handle_allocation};
use crate::request::Request;
use crate::{CapturedHandle, Win32Error};

const HEADER: usize = offset_of!(VOLUME_DISK_EXTENTS, Extents);
const EXTENT: usize = size_of::<DISK_EXTENT>();

/// The system volume as a device path, opened with no access, which an ordinary account can do.
fn system_volume(overlapped: bool) -> File {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(if overlapped { FILE_FLAG_OVERLAPPED } else { 0 })
        .open(format!(r"\\.\{drive}"))
        .expect("open the system volume with no access")
}

fn request_for(file: &File) -> QueryVolumeDiskExtents {
    QueryVolumeDiskExtents::new(CapturedHandle::capture(file.as_handle()).expect("capture"))
}

fn extent(disk_number: u32, starting_offset: u64, length: u64) -> DiskExtent {
    DiskExtent {
        disk_number,
        starting_offset,
        length,
    }
}

/// The bytes of a reply that holds `extents`, as the device lays them out.
fn reply(extents: &[(u32, i64, i64)]) -> Vec<u8> {
    let mut bytes = vec![0_u8; answer_size(extents.len()).expect("a small count")];
    let count = u32::try_from(extents.len()).expect("a small count");
    put(
        &mut bytes,
        offset_of!(VOLUME_DISK_EXTENTS, NumberOfDiskExtents),
        &count.to_ne_bytes(),
    );
    for (index, (disk, start, length)) in extents.iter().enumerate() {
        let at = HEADER + index * EXTENT;
        put(
            &mut bytes,
            at + offset_of!(DISK_EXTENT, DiskNumber),
            &disk.to_ne_bytes(),
        );
        put(
            &mut bytes,
            at + offset_of!(DISK_EXTENT, StartingOffset),
            &start.to_ne_bytes(),
        );
        put(
            &mut bytes,
            at + offset_of!(DISK_EXTENT, ExtentLength),
            &length.to_ne_bytes(),
        );
    }
    bytes
}

fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
    bytes[at..at + value.len()].copy_from_slice(value);
}

/// `count` extents on distinct disks with distinct offsets and lengths, so a field read from the
/// wrong place shows.
fn runs(count: u32) -> Vec<(u32, i64, i64)> {
    (0..count)
        .map(|index| {
            let index = i64::from(index);
            (
                u32::try_from(index).expect("small") + 1,
                0x1000 * (index + 1),
                0x100_0000 * (index + 2),
            )
        })
        .collect()
}

fn expected(runs: &[(u32, i64, i64)]) -> Vec<DiskExtent> {
    runs.iter()
        .map(|&(disk, start, length)| {
            extent(
                disk,
                u64::try_from(start).expect("non-negative"),
                u64::try_from(length).expect("non-negative"),
            )
        })
        .collect()
}

/// A device holding `held`, which answers as the real one was measured to: a buffer too small for
/// even one extent is `ERROR_INVALID_PARAMETER`, one too small for all of them is
/// `ERROR_MORE_DATA` with the count written (the documented half, which a machine of one disk
/// cannot show), and otherwise the whole reply is written. Records how big each buffer was.
struct Device {
    held: Vec<(u32, i64, i64)>,
    asked: Vec<usize>,
}

impl Device {
    fn new(held: Vec<(u32, i64, i64)>) -> Self {
        Self {
            held,
            asked: Vec::new(),
        }
    }

    fn answer(&mut self, buffer: &mut [u8]) -> Result<u32, Win32Error> {
        self.asked.push(buffer.len());
        if buffer.len() < HEADER + EXTENT {
            return Err(Win32Error::from_code(ERROR_INVALID_PARAMETER));
        }
        let whole = reply(&self.held);
        if buffer.len() < whole.len() {
            let header = &whole[..HEADER];
            buffer[..HEADER].copy_from_slice(header);
            return Err(Win32Error::from_code(ERROR_MORE_DATA));
        }
        buffer[..whole.len()].copy_from_slice(&whole);
        Ok(u32::try_from(whole.len()).expect("small"))
    }
}

// --- The real device -----------------------------------------------------------------------

#[test]
fn a_real_volume_reports_at_least_one_run_of_a_disk() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume(false);

    let extents = request_for(&volume).perform().expect("query the volume");

    assert!(!extents.is_empty(), "a volume on this machine has a run");
    assert!(
        extents.iter().all(|run| run.length() > 0),
        "a run has a length: {extents:?}"
    );
    eprintln!("the system volume: {extents:?}");
}

#[test]
fn performing_twice_and_through_a_clone_gives_the_same_answer() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume(false);
    let request = request_for(&volume);

    let first = request.perform().expect("query");
    let copy = request.try_clone().expect("duplicate the handle");
    drop(request);
    drop(volume);

    assert_eq!(copy.perform().expect("query the copy"), first);
}

#[test]
fn an_overlapped_volume_handle_gets_the_same_answer_as_a_plain_one() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let plain = system_volume(false);
    let overlapped = system_volume(true);

    assert_eq!(
        request_for(&plain).perform().expect("plain"),
        request_for(&overlapped).perform().expect("overlapped")
    );
}

#[test]
fn a_buffer_with_no_room_for_one_extent_is_refused_as_an_invalid_parameter() {
    // Measured, and not what the documentation leads one to expect: a buffer smaller than the count
    // and one extent is `ERROR_INVALID_PARAMETER` on the machine measured, where
    // `ERROR_INSUFFICIENT_BUFFER` might have been. The fake device answers the same, and this is
    // what shows it does. It is also why the entry never asks with room for fewer than one extent.
    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume(false);

    for size in [0_usize, 1, HEADER - 1, HEADER, HEADER + EXTENT - 1] {
        let mut buffer = vec![0_u8; size];
        let outcome = device_control(
            volume.as_raw_handle(),
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            &[],
            &mut buffer,
        )
        .map_err(|error| error.code());
        assert_eq!(
            outcome,
            Err(ERROR_INVALID_PARAMETER),
            "a buffer of {size} bytes"
        );
    }

    // With room for one extent the device answers, whether with the one run or, on a machine
    // whose system volume spans disks, with a count to ask again at.
    let mut buffer = vec![0_u8; HEADER + EXTENT];
    let outcome = device_control(
        volume.as_raw_handle(),
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        &[],
        &mut buffer,
    )
    .map_err(|error| error.code());
    assert!(
        matches!(outcome, Ok(_) | Err(ERROR_MORE_DATA)),
        "room for one extent: {outcome:?}"
    );
}

#[test]
fn a_handle_that_is_not_a_volume_is_the_devices_own_error() {
    let _allocating = handle_allocation().read().expect("not poisoned");
    let fixture = Fixture::new("extents-file");
    let file = fixture.open_file();

    match request_for(&file).perform() {
        Err(DiskExtentsError::Win32(error)) => eprintln!("an ordinary file: {error}"),
        other => panic!("an ordinary file is not a volume, but gave {other:?}"),
    }

    let nul = File::open("NUL").expect("open the NUL device");
    match request_for(&nul).perform() {
        Err(DiskExtentsError::Win32(error)) => eprintln!("the NUL device: {error}"),
        other => panic!("the NUL device is not a volume, but gave {other:?}"),
    }
}

#[test]
fn the_request_owns_its_handle_and_is_performable_through_the_trait() {
    fn through_the_seam<R: Request<Output = Vec<DiskExtent>, Error = DiskExtentsError>>(
        request: &R,
    ) -> Vec<DiskExtent> {
        request.perform().expect("perform")
    }

    let _allocating = handle_allocation().read().expect("not poisoned");
    let volume = system_volume(false);
    let request = request_for(&volume);

    assert_ne!(request.handle().raw(), volume.as_raw_handle());
    assert_eq!(
        through_the_seam(&request),
        request.perform().expect("perform")
    );
}

// --- A device with more extents than the first answer holds ---------------------------------

#[test]
fn a_volume_that_fits_the_first_answer_costs_one_call() {
    for count in [1_u32, 2, 7, 8] {
        let held = runs(count);
        let mut device = Device::new(held.clone());

        let got = extents_via(FIRST_EXTENTS, |buffer| device.answer(buffer)).expect("answer");

        assert_eq!(got, expected(&held), "{count} extents");
        assert_eq!(
            device.asked.len(),
            1,
            "{count} extents took more than one call"
        );
    }
}

#[test]
fn a_volume_that_does_not_fit_is_asked_again_at_the_reported_count() {
    for count in [9_u32, 10, 20, 100] {
        let held = runs(count);
        let mut device = Device::new(held.clone());

        let got = extents_via(FIRST_EXTENTS, |buffer| device.answer(buffer)).expect("answer");

        assert_eq!(got, expected(&held), "{count} extents");
        assert_eq!(
            device.asked,
            vec![
                answer_size(FIRST_EXTENTS).expect("small"),
                answer_size(usize::try_from(count).expect("small")).expect("small")
            ],
            "{count} extents: the second buffer is the size the device named"
        );
    }
}

#[test]
fn a_first_answer_of_no_room_is_given_room_for_one_extent() {
    let held = runs(1);
    let mut device = Device::new(held.clone());

    let got = extents_via(0, |buffer| device.answer(buffer)).expect("answer");

    assert_eq!(got, expected(&held));
    assert_eq!(device.asked, vec![answer_size(1).expect("small")]);
}

#[test]
fn an_empty_volume_is_an_empty_answer() {
    let mut device = Device::new(Vec::new());

    // The fake refuses less than one extent's room, as the device does, and the entry always asks
    // for that much, so a volume with none is the header alone within a buffer that has room.
    let got = extents_via(FIRST_EXTENTS, |buffer| device.answer(buffer)).expect("answer");

    assert_eq!(got, Vec::new());
}

#[test]
fn a_count_that_keeps_growing_is_unstable_after_the_bounded_attempts() {
    let mut asked = 0_usize;
    let mut next = FIRST_EXTENTS;

    let outcome = extents_via(FIRST_EXTENTS, |buffer| {
        asked += 1;
        next += 1;
        // Always one more than the buffer holds, so no buffer is ever big enough.
        let header = reply(&runs(u32::try_from(next).expect("small")))[..HEADER].to_vec();
        buffer[..HEADER].copy_from_slice(&header);
        Err(Win32Error::from_code(ERROR_MORE_DATA))
    });

    assert!(
        matches!(outcome, Err(DiskExtentsError::Unstable { attempts }) if attempts == MAX_ATTEMPTS),
        "{outcome:?}"
    );
    assert_eq!(
        asked, MAX_ATTEMPTS,
        "the retry is bounded, and uses its bound"
    );
}

#[test]
fn a_reply_that_needs_more_room_but_counts_none_still_leaves_room_for_one() {
    let mut sizes = Vec::new();

    let _ = extents_via(FIRST_EXTENTS, |buffer| {
        sizes.push(buffer.len());
        // The count stays zero: the header is left as it was.
        Err(Win32Error::from_code(ERROR_MORE_DATA))
    });

    assert!(
        sizes
            .iter()
            .all(|size| *size >= answer_size(1).expect("small")),
        "a buffer with no room for an extent was asked with: {sizes:?}"
    );
}

#[test]
fn any_other_failure_is_the_devices_own_code_and_ends_the_attempts() {
    let mut asked = 0_usize;

    let outcome = extents_via(FIRST_EXTENTS, |_| {
        asked += 1;
        Err(Win32Error::from_code(ERROR_ACCESS_DENIED))
    });

    match outcome {
        Err(DiskExtentsError::Win32(error)) => assert_eq!(error.code(), ERROR_ACCESS_DENIED),
        other => panic!("expected the device's own code, got {other:?}"),
    }
    assert_eq!(asked, 1, "a refusal is not retried");
}

// --- What the entry does with a reply --------------------------------------------------------

#[test]
fn a_reply_is_read_field_by_field_from_where_the_device_put_each() {
    let held = runs(3);
    let bytes = reply(&held);

    let got = decode(&bytes, u32::try_from(bytes.len()).expect("small")).expect("a whole reply");

    assert_eq!(got, expected(&held));
    assert_eq!(got[1].disk_number(), 2);
    assert_eq!(got[1].starting_offset(), 0x2000);
    assert_eq!(got[1].length(), 0x300_0000);
}

#[test]
fn a_reply_that_exactly_fills_what_was_written_is_accepted() {
    let held = runs(2);
    let bytes = reply(&held);
    let exact = u32::try_from(bytes.len()).expect("small");

    assert_eq!(
        decode(&bytes, exact).expect("an exact fit"),
        expected(&held)
    );
    // And only the bytes written are read, whatever else the buffer holds.
    let mut roomy = bytes.clone();
    roomy.extend_from_slice(&[0xFF; 64]);
    assert_eq!(
        decode(&roomy, exact).expect("trailing room"),
        expected(&held)
    );
}

#[test]
fn a_reply_that_is_not_what_it_counts_is_not_read() {
    let held = runs(2);
    let whole = reply(&held);
    let size = |bytes: &[u8]| u32::try_from(bytes.len()).expect("small");
    let malformed = |outcome: Result<Vec<DiskExtent>, DiskExtentsError>| {
        matches!(outcome, Err(DiskExtentsError::Malformed { .. }))
    };

    // Shorter than its own count.
    for written in 0..4_u32 {
        assert!(malformed(decode(&whole, written)), "written {written}");
    }
    // A count the written bytes cannot hold.
    assert!(
        malformed(decode(&whole, size(&whole) - 1)),
        "one byte short"
    );
    assert!(malformed(decode(
        &whole,
        u32::try_from(HEADER).expect("small")
    )));
    // More written than the buffer has room for.
    assert!(malformed(decode(&whole, size(&whole) + 1)));
    assert!(malformed(decode(&whole, u32::MAX)));
    // A negative start, and a negative length.
    let negative_start = reply(&[(1, -1, 4096)]);
    assert!(malformed(decode(&negative_start, size(&negative_start))));
    let negative_length = reply(&[(1, 0, -1)]);
    assert!(malformed(decode(&negative_length, size(&negative_length))));
}

#[test]
fn the_largest_values_a_disk_can_report_are_carried() {
    let bytes = reply(&[(u32::MAX, i64::MAX, i64::MAX)]);

    let got = decode(&bytes, u32::try_from(bytes.len()).expect("small")).expect("whole");

    assert_eq!(
        got,
        vec![extent(
            u32::MAX,
            i64::MAX.unsigned_abs(),
            i64::MAX.unsigned_abs()
        )]
    );
}

#[test]
fn a_buffer_the_machine_cannot_supply_is_reported_as_out_of_memory() {
    // `isize::MAX` is the most any allocation can name, and no machine has it.
    match zeroed(usize::try_from(isize::MAX).expect("fits")) {
        Err(DiskExtentsError::Win32(error)) => assert_eq!(error.code(), ERROR_NOT_ENOUGH_MEMORY),
        other => panic!("expected out of memory, got {other:?}"),
    }
    assert_eq!(zeroed(16).expect("a small buffer"), vec![0_u8; 16]);
}

// --- Display and errors ----------------------------------------------------------------------

#[test]
fn errors_name_the_call_and_keep_the_raw_code() {
    let refused = DiskExtentsError::from(Win32Error::from_code(1));
    assert!(
        refused
            .to_string()
            .starts_with("IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: ")
    );
    let source = std::error::Error::source(&refused).expect("a Win32 source");
    assert_eq!(
        source
            .downcast_ref::<Win32Error>()
            .map(|error| error.code()),
        Some(1)
    );

    let malformed = DiskExtentsError::Malformed {
        what: "an answer that lies",
    };
    assert!(malformed.to_string().contains("an answer that lies"));
    assert!(std::error::Error::source(&malformed).is_none());

    let unstable = DiskExtentsError::Unstable { attempts: 4 };
    assert!(unstable.to_string().contains('4'));
    assert!(std::error::Error::source(&unstable).is_none());
}

#[test]
fn an_extent_displays_its_disk_offset_and_length() {
    assert_eq!(
        extent(3, 4096, 8192).to_string(),
        "disk 3 at 4096 for 8192 bytes"
    );
}
