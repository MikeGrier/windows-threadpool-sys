// Copyright (c) 2026 Mike Grier
//! A file opened unbuffered, through an instance (DI-D-60): what the open mode warrants is
//! carried through to the operations the instance runs, against the kernel.

use std::time::{Duration, Instant};

use io_buffers::AlignedBuf;

use super::{QUEUE, TempFile};
use crate::dioring::{DeliveryMode, Dioring, Setup, TargetSetup};
use crate::windows_backend::{FileOptions, OpenedFile, ReadWrite, Unbuffered, Win32FileTarget};
use durable_ioring::backend::RefusalCause;
use durable_ioring::{
    DurableRing, Entry, IoTarget, IoTargetKey, LineageHandle, OpCompletion, Outcome, PushRefusal,
    WarrantyClass, WriteCaching, WriteOptions,
};

const FILE: IoTargetKey = IoTargetKey(1);
const SECTOR: usize = Unbuffered::DEFAULT_SECTOR_SIZE as usize;
const BOUND: Duration = Duration::from_secs(10);

type AlignedRing = Dioring<AlignedBuf, u64, u32>;

fn unbuffered(temp: &TempFile, options: FileOptions<ReadWrite>) -> Win32FileTarget {
    Win32FileTarget::new(OpenedFile::<Unbuffered>::open(&temp.0, options).expect("open unbuffered"))
}

fn instance(temp: &TempFile) -> (AlignedRing, LineageHandle) {
    instance_with(temp, FileOptions::new().read().write())
}

fn instance_with(temp: &TempFile, options: FileOptions<ReadWrite>) -> (AlignedRing, LineageHandle) {
    let target = unbuffered(temp, options);
    assert_eq!(target.warranty(), WarrantyClass::SharedDevice);
    assert!(target.capabilities().write_through());
    AlignedRing::new(Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files: vec![TargetSetup { key: FILE, target }],
        buffers: Vec::<Vec<u8>>::new(),
        provider: None,
        diagnostics: None,
        delivery: DeliveryMode::Pool(None),
    })
    .expect("build an instance over an unbuffered file")
}

fn sector(byte: u8) -> AlignedBuf {
    AlignedBuf::from_bytes(&vec![byte; SECTOR], SECTOR).expect("an aligned buffer")
}

/// The next completion, which must be of an operation.
fn completion(ring: &mut AlignedRing) -> OpCompletion<crate::WindowsIds<u64>, AlignedBuf, u32> {
    let started = Instant::now();
    loop {
        match ring.pop().expect("pop") {
            Some(Entry::Op(completion)) => return completion,
            Some(other) => panic!("expected a completion, got {other:?}"),
            None => {
                assert!(started.elapsed() < BOUND, "nothing arrived");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

// A cache hint is passed to an unbuffered file and not refused (`DI-14.1`), so the ring has to be
// able to run it: every way of asking for the two hints, against the kernel and through the ring.
#[test]
fn an_unbuffered_file_opened_with_cache_hints_runs_through_the_ring() {
    for (random, sequential) in [(false, false), (true, false), (false, true), (true, true)] {
        let temp = TempFile::new("unbuffered-hints");
        let options = FileOptions::new()
            .read()
            .write()
            .random_access(random)
            .sequential_scan(sequential);
        let (mut ring, lineage) = instance_with(&temp, options);
        ring.write_with(
            FILE,
            0,
            sector(3),
            lineage.at(1),
            8,
            WriteOptions::new().caching(WriteCaching::WriteThrough),
        )
        .expect("push an aligned write-through write");
        let done = completion(&mut ring);
        assert!(
            matches!(done.outcome, Outcome::Transferred(n) if n as usize == SECTOR),
            "hints {random}, {sequential}: {:?}",
            done.outcome
        );
        assert_eq!(
            std::fs::read(&temp.0).expect("read back")[..SECTOR],
            vec![3u8; SECTOR]
        );
    }
}

#[test]
fn a_per_write_write_through_write_completes_on_an_unbuffered_file() {
    // The cached counterpart is `the_caching_choice_reaches_the_kernel`, where the kernel refuses
    // the same flag (error 509); this is why an unbuffered file offers the capability and a cached
    // one does not.
    let temp = TempFile::new("unbuffered-through");
    let (mut ring, lineage) = instance(&temp);
    ring.write_with(
        FILE,
        0,
        sector(9),
        lineage.at(1),
        5,
        WriteOptions::new().caching(WriteCaching::WriteThrough),
    )
    .expect("push an aligned write-through write");
    let done = completion(&mut ring);
    assert!(
        matches!(done.outcome, Outcome::Transferred(n) if n as usize == SECTOR),
        "{:?}",
        done.outcome
    );
    assert_eq!(done.buffer.as_deref().map(<[u8]>::len), Some(SECTOR));
    assert_eq!(
        std::fs::read(&temp.0).expect("read back")[..SECTOR],
        vec![9u8; SECTOR]
    );
}

#[test]
fn an_operation_off_the_sector_is_refused_before_the_kernel_is_asked() {
    let temp = TempFile::new("unbuffered-refused");
    let (mut ring, lineage) = instance(&temp);
    for offset in [1, 512, SECTOR as u64 + 1] {
        let refused = ring
            .write(FILE, offset, sector(1), lineage.at(1), 6)
            .expect_err("off the sector");
        assert!(
            matches!(refused.reason, PushRefusal::Backend(RefusalCause::Invalid)),
            "offset {offset}: {:?}",
            refused.reason
        );
        assert!(refused.buffer.is_some(), "the buffer comes back");
    }
    let short = AlignedBuf::from_bytes(&[0; 512], SECTOR).expect("aligned, but short");
    let refused = ring
        .write(FILE, 0, short, lineage.at(1), 7)
        .expect_err("a length off the sector");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::Invalid)
    ));
}
