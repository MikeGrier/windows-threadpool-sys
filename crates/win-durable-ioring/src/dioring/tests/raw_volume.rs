// Copyright (c) 2026 Mike Grier
//! A raw volume through an instance (DI-D-61), against a real volume: a scratch virtual disk with
//! one raw partition, created with `diskpart` ([`crate::scratch_volume`]).
//!
//! Opening a volume for writing and creating a virtual disk need an administrator, so these tests
//! are `#[ignore]`d and run elevated:
//!
//! ```text
//! cargo test -p win-durable-ioring --lib -- --include-ignored raw_volume
//! ```
//!
//! What no in-memory test can show is here: that the volume's disks are found, that its sector is
//! the alignment the kernel holds a transfer to, that a write reaches it and is made durable, and
//! that the lock is taken and given up.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::io::AsHandle;
use std::time::{Duration, Instant};

use io_buffers::AlignedBuf;

use super::QUEUE;
use crate::dioring::{DeliveryMode, Dioring, Setup, TargetSetup};
use crate::scratch_volume::ScratchVolume;
use crate::windows_backend::{
    OpenedVolume, ReadOnly, ShareMode, SharingViolation, Unbuffered, VolumeOptions,
    Win32FileTarget, disk_flush_domain,
};
use durable_ioring::backend::RefusalCause;
use durable_ioring::{
    DurableRing, Entry, EntryOf, IoTarget, IoTargetKey, LineageHandle, Outcome, PushRefusal,
    WarrantyClass, WriteCaching, WriteOptions,
};

const VOLUME: IoTargetKey = IoTargetKey(1);
const BOUND: Duration = Duration::from_secs(30);
const MEGABYTES: u32 = 64;

type RawRing = Dioring<AlignedBuf, u64, u32>;

fn raw_instance(volume: OpenedVolume<Unbuffered>) -> (RawRing, LineageHandle) {
    RawRing::new(Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files: vec![TargetSetup::new(
            VOLUME,
            Win32FileTarget::from_volume(volume),
        )],
        buffers: Vec::<Vec<u8>>::new(),
        provider: None,
        diagnostics: None,
        delivery: DeliveryMode::Pool(None),
    })
    .expect("build an instance over the raw volume")
}

fn next_entry(ring: &mut RawRing) -> EntryOf<RawRing> {
    let started = Instant::now();
    loop {
        if let Some(entry) = ring.pop().expect("pop") {
            return entry;
        }
        assert!(started.elapsed() < BOUND, "nothing arrived");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn sector_of(fill: u8, sector: usize) -> AlignedBuf {
    AlignedBuf::from_bytes(&vec![fill; sector], sector).expect("an aligned buffer")
}

fn transferred(entry: EntryOf<RawRing>) -> u32 {
    match entry {
        Entry::Op(completion) => match completion.outcome {
            Outcome::Transferred(n) => n,
            other => panic!("expected a transfer, got {other:?}"),
        },
        other => panic!("expected a completion, got {other:?}"),
    }
}

/// The first `length` bytes of the volume, read through a handle of the test's own.
fn read_back(device_path: &str, length: usize) -> Vec<u8> {
    let mut volume = OpenOptions::new()
        .read(true)
        .open(device_path)
        .expect("open the volume for reading");
    volume.seek(SeekFrom::Start(0)).expect("seek");
    let mut bytes = vec![0; length];
    volume.read_exact(&mut bytes).expect("read the volume");
    bytes
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_raw_volume_opened_for_reading_only_reads_and_is_refused_a_write_by_the_kernel() {
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let volume = OpenedVolume::<Unbuffered, ReadOnly>::open(&scratch.path(), VolumeOptions::new())
        .expect("open the raw volume for reading only");
    let sector = volume.geometry().offset_alignment() as usize;
    let mut through = File::from(volume.as_handle().try_clone_to_owned().expect("duplicate"));

    let mut buffer = AlignedBuf::zeroed(sector, sector).expect("an aligned buffer");
    through
        .read_exact(buffer.as_mut_slice())
        .expect("a read of the first sector");
    let refused = through
        .seek(SeekFrom::Start(0))
        .and_then(|_| through.write_all(sector_of(1, sector).as_slice()));
    assert_eq!(
        refused
            .expect_err("a write the access does not cover")
            .raw_os_error(),
        Some(5),
        "ERROR_ACCESS_DENIED"
    );
    // A volume opened for reading only still has a data right, so it is a target.
    let _target = Win32FileTarget::from_volume(volume);
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_raw_volumes_disks_and_sector_are_found_and_its_target_reports_them() {
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let volume = OpenedVolume::<Unbuffered>::open(&scratch.path(), VolumeOptions::new())
        .expect("open the raw volume");
    assert!(!volume.is_locked());

    let extents = volume.disk_extents();
    assert_eq!(
        extents.len(),
        1,
        "a volume on one virtual disk: {extents:?}"
    );
    assert!(extents[0].length > 0);
    let disk = extents[0].disk_number;

    let sector = volume.geometry().offset_alignment();
    assert!(sector.is_power_of_two() && sector >= 512, "{sector}");

    let target = Win32FileTarget::from_volume(volume);
    assert_eq!(target.warranty(), WarrantyClass::SharedDevice);
    assert_eq!(target.flush_domains(), [disk_flush_domain(disk)]);
    assert!(target.capabilities().write_through());
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_sector_written_to_a_raw_volume_is_made_durable_and_is_on_the_volume() {
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let volume = OpenedVolume::<Unbuffered>::open(&scratch.path(), VolumeOptions::new())
        .expect("open the raw volume");
    let sector = volume.geometry().offset_alignment() as usize;
    let (mut ring, lineage) = raw_instance(volume);

    // A region of the consumer's own, the third sector: the offset is what the consumer chose.
    let region = 3 * sector as u64;
    ring.write_with(
        VOLUME,
        region,
        sector_of(0xAB, sector),
        lineage.at(1),
        1,
        WriteOptions::new(),
    )
    .expect("push an aligned write");
    assert_eq!(transferred(next_entry(&mut ring)) as usize, sector);

    // Off the sector, in offset or in length, the backend refuses before the kernel is asked.
    for offset in [1, sector as u64 / 2] {
        let refused = ring
            .write(
                VOLUME,
                region + offset,
                sector_of(1, sector),
                lineage.at(1),
                2,
            )
            .expect_err("off the sector");
        assert!(matches!(
            refused.reason,
            PushRefusal::Backend(RefusalCause::Invalid)
        ));
    }

    ring.make_durable_through(lineage.at(1))
        .expect("seal the epoch");
    match next_entry(&mut ring) {
        Entry::Durable { through } => assert_eq!(through.id, 1),
        other => panic!("expected Durable, got {other:?}"),
    }

    let on_volume = read_back(&scratch.device_path(), region as usize + sector);
    assert!(on_volume[..region as usize].iter().all(|&b| b == 0));
    assert!(on_volume[region as usize..].iter().all(|&b| b == 0xAB));
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_write_through_write_completes_on_a_raw_volume() {
    // The capability the target offers because it is unbuffered: the kernel refuses the flag on a
    // cached handle, and takes it on this one.
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let volume = OpenedVolume::<Unbuffered>::open(&scratch.path(), VolumeOptions::new())
        .expect("open the raw volume");
    let sector = volume.geometry().offset_alignment() as usize;
    let (mut ring, lineage) = raw_instance(volume);
    ring.write_with(
        VOLUME,
        0,
        sector_of(0x5A, sector),
        lineage.at(1),
        7,
        WriteOptions::new().caching(WriteCaching::WriteThrough),
    )
    .expect("push a write-through write");
    assert_eq!(transferred(next_entry(&mut ring)) as usize, sector);
    assert!(
        read_back(&scratch.device_path(), sector)
            .iter()
            .all(|&b| b == 0x5A)
    );
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_locked_raw_volume_is_written_and_the_lock_is_given_up_when_it_closes() {
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let path = scratch.path();

    let locked = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new().lock(true))
        .expect("open and lock");
    assert!(locked.is_locked());
    let sector = locked.geometry().offset_alignment() as usize;
    let (mut ring, lineage) = raw_instance(locked);
    ring.write_with(
        VOLUME,
        0,
        sector_of(0x11, sector),
        lineage.at(1),
        1,
        WriteOptions::new(),
    )
    .expect("push a write to the locked volume");
    assert_eq!(transferred(next_entry(&mut ring)) as usize, sector);

    // The instance ends, its handle closes, and the volume can be locked again.
    drop(ring);
    OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new().lock(true))
        .expect("the lock was released when the handle closed");
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn an_exclusive_raw_volume_refuses_a_second_open_until_it_closes() {
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let path = scratch.path();
    let exclusive =
        OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new().share(ShareMode::NONE))
            .expect("open exclusively");
    let refused = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new())
        .expect_err("an exclusive open is open");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::ResourceBusy,
        "{refused}"
    );
    assert!(
        refused
            .get_ref()
            .is_some_and(|inner| inner.is::<SharingViolation>()),
        "{refused}"
    );
    drop(exclusive);
    OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new())
        .expect("nothing holds the volume now");
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_volume_can_be_locked_and_exclusive_at_once_and_the_share_mode_still_judges_a_second_open() {
    // DI-13.12. The share mode is judged by the open and the lock is a device control made after it,
    // so the two are expected to be independent; this is the measurement, and what it shows is
    // written in DI-D-64, not assumed from here.
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let path = scratch.path();
    let held = OpenedVolume::<Unbuffered>::open(
        &path,
        VolumeOptions::new().lock(true).share(ShareMode::NONE),
    )
    .expect("open exclusively and lock");
    assert!(held.is_locked());
    let refused = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new())
        .expect_err("an exclusive open is open, locked or not");
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::ResourceBusy,
        "{refused}"
    );
    drop(held);

    // Locked and shared: what a second, plain open of the volume meets is recorded, not asserted.
    let locked = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new().lock(true))
        .expect("open and lock, sharing all three");
    let second = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new());
    eprintln!("a second open beside a locked, shared volume: {second:?}");
    drop((locked, second));
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_volume_with_another_handle_open_cannot_be_locked() {
    let scratch = ScratchVolume::create(MEGABYTES).expect("a scratch volume");
    let path = scratch.path();
    let held = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new()).expect("open");
    let refused = OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new().lock(true))
        .expect_err("another handle is open");
    assert!(
        refused.raw_os_error().is_some(),
        "the lock's own error: {refused}"
    );
    drop(held);
    OpenedVolume::<Unbuffered>::open(&path, VolumeOptions::new().lock(true))
        .expect("nothing else is open now");
}
