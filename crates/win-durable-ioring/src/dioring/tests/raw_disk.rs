// Copyright (c) 2026 Mike Grier
//! A whole physical disk through an instance (DI-D-62), against a real disk: a scratch virtual disk
//! with no partition and no volume on it, made with `diskpart` ([`crate::scratch_volume`]).
//!
//! Opening a disk for writing and creating a virtual disk need an administrator, so these tests are
//! `#[ignore]`d and run elevated:
//!
//! ```text
//! cargo test -p win-durable-ioring --lib -- --include-ignored raw_disk
//! ```
//!
//! **What they write to.** Only the disk `diskpart` reports for the virtual disk each test makes:
//! its number is read from `detail vdisk` for the exact file the test created, which must be named
//! once and cannot be disk 0 ([`crate::scratch_volume::associated_disk`]), and every write is to
//! that number. A test never names a disk number of its own.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::io::AsHandle;
use std::time::{Duration, Instant};

use io_buffers::AlignedBuf;

use super::QUEUE;
use crate::dioring::{DeliveryMode, Dioring, Setup, TargetSetup};
use crate::scratch_volume::ScratchDisk;
use crate::windows_backend::{
    DiskPath, OpenedDisk, ReadOnly, Unbuffered, Win32FileTarget, disk_flush_domain,
};
use durable_ioring::backend::RefusalCause;
use durable_ioring::{
    DurableRing, Entry, EntryOf, IoTarget, IoTargetKey, LineageHandle, Outcome, PushRefusal,
    WarrantyClass, WriteCaching, WriteOptions,
};

const DISK: IoTargetKey = IoTargetKey(1);
const BOUND: Duration = Duration::from_secs(30);
const MEGABYTES: u32 = 64;

type DiskRing = Dioring<AlignedBuf, u64, u32>;

fn raw_instance(disk: OpenedDisk<Unbuffered>) -> (DiskRing, LineageHandle) {
    DiskRing::new(Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files: vec![TargetSetup {
            key: DISK,
            target: Win32FileTarget::from_disk(disk),
        }],
        buffers: Vec::<Vec<u8>>::new(),
        provider: None,
        diagnostics: None,
        delivery: DeliveryMode::Pool(None),
    })
    .expect("build an instance over the raw disk")
}

fn next_entry(ring: &mut DiskRing) -> EntryOf<DiskRing> {
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

fn outcome(entry: EntryOf<DiskRing>) -> Outcome<crate::WindowsIds<u64>> {
    match entry {
        Entry::Op(completion) => completion.outcome,
        other => panic!("expected a completion, got {other:?}"),
    }
}

/// `length` bytes at `offset` of disk `disk_number`, read through a handle of the test's own.
fn read_back(disk_number: u32, offset: u64, length: usize) -> Vec<u8> {
    let mut disk = OpenOptions::new()
        .read(true)
        .open(DiskPath::number(disk_number).path())
        .expect("open the disk for reading");
    disk.seek(SeekFrom::Start(offset)).expect("seek");
    let mut bytes = vec![0; length];
    disk.read_exact(&mut bytes).expect("read the disk");
    bytes
}

fn open(scratch: &ScratchDisk) -> OpenedDisk<Unbuffered> {
    OpenedDisk::<Unbuffered>::open(DiskPath::number(scratch.disk_number()))
        .expect("open the scratch disk")
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_raw_disk_opened_for_reading_only_reads_and_is_refused_a_write_by_the_kernel() {
    let scratch = ScratchDisk::create(MEGABYTES).expect("a scratch disk");
    let disk = OpenedDisk::<Unbuffered, ReadOnly>::open(DiskPath::number(scratch.disk_number()))
        .expect("open the scratch disk for reading only");
    let sector = disk.geometry().offset_alignment() as usize;
    let mut through = File::from(disk.as_handle().try_clone_to_owned().expect("duplicate"));

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
    // A disk opened for reading only still has a data right, so it is a target.
    let _target = Win32FileTarget::from_disk(disk);
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_raw_disks_sector_is_found_and_its_target_names_the_disk() {
    let scratch = ScratchDisk::create(MEGABYTES).expect("a scratch disk");
    let disk = open(&scratch);
    assert_eq!(disk.disk_number(), scratch.disk_number());
    let sector = disk.geometry().offset_alignment();
    assert!(sector.is_power_of_two() && sector >= 512, "{sector}");
    assert_eq!(disk.geometry().length_alignment(), sector);
    assert_eq!(disk.geometry().buffer_alignment(), sector);

    let target = Win32FileTarget::from_disk(disk);
    assert_eq!(target.warranty(), WarrantyClass::SharedDevice);
    assert_eq!(
        target.flush_domains(),
        [disk_flush_domain(scratch.disk_number())]
    );
    assert!(target.capabilities().write_through());
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn sectors_written_to_a_raw_disk_are_made_durable_and_are_on_the_disk() {
    let scratch = ScratchDisk::create(MEGABYTES).expect("a scratch disk");
    let disk = open(&scratch);
    let sector = disk.geometry().offset_alignment() as usize;
    let (mut ring, lineage) = raw_instance(disk);

    // The first sector, and one a mebibyte in: offsets are the consumer's to choose.
    let at = [0, 1024 * 1024];
    for (context, offset) in at.into_iter().enumerate() {
        ring.write_with(
            DISK,
            offset,
            sector_of(0xC0 + context as u8, sector),
            lineage.at(1),
            context as u32,
            WriteOptions::new(),
        )
        .expect("push an aligned write");
    }
    for _ in at {
        assert!(
            matches!(outcome(next_entry(&mut ring)), Outcome::Transferred(n) if n as usize == sector)
        );
    }

    ring.make_durable_through(lineage.at(1))
        .expect("seal the epoch");
    match next_entry(&mut ring) {
        Entry::Durable { through } => assert_eq!(through.id, 1),
        other => panic!("expected Durable, got {other:?}"),
    }

    for (context, offset) in at.into_iter().enumerate() {
        let on_disk = read_back(scratch.disk_number(), offset, sector);
        assert!(
            on_disk.iter().all(|&b| b == 0xC0 + context as u8),
            "the sector at {offset}"
        );
    }
    // Between them nothing was written.
    assert!(
        read_back(scratch.disk_number(), sector as u64, sector)
            .iter()
            .all(|&b| b == 0)
    );
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn the_last_sector_of_a_raw_disk_is_accepted_and_the_one_past_it_is_not() {
    let scratch = ScratchDisk::create(MEGABYTES).expect("a scratch disk");
    let disk = open(&scratch);
    let sector = disk.geometry().offset_alignment() as usize;
    let size = scratch.bytes();
    let last = size - sector as u64;
    let (mut ring, lineage) = raw_instance(disk);

    ring.write_with(
        DISK,
        last,
        sector_of(0x77, sector),
        lineage.at(1),
        1,
        WriteOptions::new(),
    )
    .expect("push the last sector");
    assert!(
        matches!(outcome(next_entry(&mut ring)), Outcome::Transferred(n) if n as usize == sector),
        "the last sector of the disk"
    );
    assert!(
        read_back(scratch.disk_number(), last, sector)
            .iter()
            .all(|&b| b == 0x77)
    );

    // One sector further is past the end of the disk: the kernel does not take it, whatever it
    // answers with, and nothing is transferred.
    ring.write_with(
        DISK,
        size,
        sector_of(0x88, sector),
        lineage.at(1),
        2,
        WriteOptions::new(),
    )
    .expect("the backend cannot know the disk's size, so it pushes it");
    let past = outcome(next_entry(&mut ring));
    assert!(
        !matches!(past, Outcome::Transferred(n) if n > 0),
        "a write past the end of the disk transferred: {past:?}"
    );
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn an_operation_off_the_sector_is_refused_before_the_disk_is_asked() {
    let scratch = ScratchDisk::create(MEGABYTES).expect("a scratch disk");
    let disk = open(&scratch);
    let sector = disk.geometry().offset_alignment() as usize;
    let (mut ring, lineage) = raw_instance(disk);
    for offset in [1, sector as u64 / 2, sector as u64 + 1] {
        let refused = ring
            .write(DISK, offset, sector_of(1, sector), lineage.at(1), 3)
            .expect_err("off the sector");
        assert!(
            matches!(refused.reason, PushRefusal::Backend(RefusalCause::Invalid)),
            "offset {offset}"
        );
    }
    let short = AlignedBuf::from_bytes(&vec![0; sector / 2], sector).expect("aligned, but short");
    let refused = ring
        .write(DISK, 0, short, lineage.at(1), 4)
        .expect_err("a length off the sector");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Invalid)
    ));
    // Nothing reached the disk: its first sector is as the disk was made.
    assert!(
        read_back(scratch.disk_number(), 0, sector)
            .iter()
            .all(|&b| b == 0)
    );
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_write_through_write_completes_on_a_raw_disk() {
    let scratch = ScratchDisk::create(MEGABYTES).expect("a scratch disk");
    let disk = open(&scratch);
    let sector = disk.geometry().offset_alignment() as usize;
    let (mut ring, lineage) = raw_instance(disk);
    ring.write_with(
        DISK,
        0,
        sector_of(0x5A, sector),
        lineage.at(1),
        5,
        WriteOptions::new().caching(WriteCaching::WriteThrough),
    )
    .expect("push a write-through write");
    assert!(
        matches!(outcome(next_entry(&mut ring)), Outcome::Transferred(n) if n as usize == sector)
    );
    assert!(
        read_back(scratch.disk_number(), 0, sector)
            .iter()
            .all(|&b| b == 0x5A)
    );
}
