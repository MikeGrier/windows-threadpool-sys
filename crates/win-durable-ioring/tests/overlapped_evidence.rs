// Copyright (c) 2026 Mike Grier
//! Evidence for DI-14.7: the ring on a handle opened with `FILE_FLAG_OVERLAPPED`, which the typed
//! surface of `win-durable-ioring` does not offer, against the same handle opened without it.
//!
//! # Why this is an integration test
//!
//! It opens real files and drives a real `IoRing`, an operating-system boundary this repository's
//! quality rule places in `tests/`.
//!
//! # What it does
//!
//! Each of four handles (cached or unbuffered, with the flag or without) is opened through the
//! request crate's `OpenFile` with the flag in its raw flags, and the same sequence is driven through
//! the ring: a write, a flush, a read, a read at the end of the file, a read across it, a write that
//! fails, an unaligned read, four reads in one batch, and a read through a registered file. What is
//! compared is each completion's result and byte count, the order a batch completes in, whether the
//! ring accepted the handle, and whether each completion was already there when the submit returned.
//!
//! The measurements are in `DI-D-64`'s "What building DI-14.7 settled" in `durable-ioring/DESIGN-NOTES.md`.
//! This file asserts the parts that do not depend on the host: each step's result, that a plain handle
//! completes inline, and that a batch's completions are the four queued. Whether an overlapped handle
//! completes a read inline is host-dependent and is reported there, not asserted here.

#![cfg(windows)]

use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::time::Duration;

use io_buffers::AlignedBuf;
use windows_ioring_sys::{
    Batch, FlushCoverage, FlushMode, IoRing, IoRingErrorExt, PushOptions, SharedHandle,
    WriteCaching,
};
use windows_namespace_request_sys::OpenFile;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED, FILE_READ_DATA, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_DATA, OPEN_ALWAYS, OPEN_EXISTING,
};
use wtf_string::Wtf16String;

const BOUND: Duration = Duration::from_secs(10);
const SECTOR: usize = 4096;

type Ring = IoRing<AlignedBuf, usize>;

/// What one operation's completion said: a byte count, or the `HRESULT` of its failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Done(usize),
    Failed(i32),
}

/// One step of the sequence, and whether its completion was already poppable when the submit
/// returned, which is what a synchronous handle does.
#[derive(Clone, Copy, Debug)]
struct Step {
    label: &'static str,
    outcome: Outcome,
    already_there: bool,
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn open(path: &Path, access: u32, disposition: u32, flags: u32) -> SharedHandle {
    let request = OpenFile::new(Wtf16String::from_os_str(path.as_os_str()))
        .expect("a path")
        .with_desired_access(access)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(disposition)
        .with_flags_and_attributes(flags);
    let handle: OwnedHandle = request.perform().expect("the open");
    SharedHandle::from(handle)
}

fn filled(byte: u8) -> AlignedBuf {
    AlignedBuf::from_bytes(&vec![byte; SECTOR], SECTOR).expect("an aligned buffer")
}

fn outcome(completion: &windows_ioring_sys::Completion) -> Outcome {
    match completion.result() {
        Ok(count) => Outcome::Done(count),
        Err(error) => Outcome::Failed(
            error
                .as_ioring_error()
                .map(|native| native.code())
                .or(error.raw_os_error())
                .expect("a code"),
        ),
    }
}

/// Submits what `push` queues and pops its one completion, noting whether it was there at once.
fn drive<F>(ring: &mut Ring, label: &'static str, push: F) -> (Step, Option<AlignedBuf>)
where
    F: FnOnce(&mut Batch<'_, AlignedBuf, usize>),
{
    {
        let mut batch = Batch::new(ring);
        push(&mut batch);
        batch.submit().expect("submit");
    }
    let (popped, already_there) = match ring.try_pop().expect("try_pop") {
        Some(popped) => (popped, true),
        None => (
            ring.pop_within(BOUND)
                .expect("pop_within")
                .expect("a completion within the bound"),
            false,
        ),
    };
    let (completion, held) = popped;
    let buffer = held.and_then(|(buffer, _extra)| buffer);
    (
        Step {
            label,
            outcome: outcome(&completion),
            already_there,
        },
        buffer,
    )
}

/// The sequence against one pair of handles, as `(steps, the order a batch of four completed in)`.
fn sequence(unbuffered: bool, overlapped: bool, tag: &str) -> (Vec<Step>, Vec<usize>) {
    let scratch = Scratch(std::env::temp_dir().join(format!(
        "durable-ioring-overlapped-{}-{tag}.tmp",
        std::process::id()
    )));
    let flags = (if unbuffered {
        FILE_FLAG_NO_BUFFERING
    } else {
        0
    }) | (if overlapped { FILE_FLAG_OVERLAPPED } else { 0 });
    let file = open(
        &scratch.0,
        FILE_READ_DATA | FILE_WRITE_DATA,
        OPEN_ALWAYS,
        flags,
    );
    let reader = open(&scratch.0, FILE_READ_DATA, OPEN_EXISTING, flags);

    let mut ring: Ring = IoRing::with_inventory(16, 32).expect("a ring");
    let mut steps = Vec::new();
    let none = PushOptions::default();

    let (step, _) = drive(&mut ring, "write A at 0", |batch| {
        batch
            .write_owned(&file, filled(0xA5), 0, 0, none, WriteCaching::Cached)
            .expect("queue");
    });
    steps.push(step);
    let (step, _) = drive(&mut ring, "write B at 4096", |batch| {
        batch
            .write_owned(
                &file,
                filled(0x5A),
                0,
                SECTOR as u64,
                none,
                WriteCaching::Cached,
            )
            .expect("queue");
    });
    steps.push(step);
    let (step, _) = drive(&mut ring, "flush", |batch| {
        batch
            .flush_owned(&file, 0, FlushCoverage::Unordered, FlushMode::Default)
            .expect("queue");
    });
    steps.push(step);

    let (step, buffer) = drive(&mut ring, "read at 0", |batch| {
        batch
            .read_owned(&file, filled(0), 0, 0, none)
            .expect("queue");
    });
    assert_eq!(buffer.expect("a buffer").as_slice(), &[0xA5; SECTOR][..]);
    steps.push(step);
    let (step, buffer) = drive(&mut ring, "read at 4096", |batch| {
        batch
            .read_owned(&file, filled(0), 0, SECTOR as u64, none)
            .expect("queue");
    });
    assert_eq!(buffer.expect("a buffer").as_slice(), &[0x5A; SECTOR][..]);
    steps.push(step);
    let (step, _) = drive(&mut ring, "read at the end of the file", |batch| {
        batch
            .read_owned(&file, filled(0), 0, 2 * SECTOR as u64, none)
            .expect("queue");
    });
    steps.push(step);
    let (step, _) = drive(&mut ring, "read across the end", |batch| {
        let buffer = AlignedBuf::zeroed(2 * SECTOR, SECTOR).expect("a buffer");
        batch
            .read_owned(&file, buffer, 0, SECTOR as u64, none)
            .expect("queue");
    });
    steps.push(step);
    let (step, _) = drive(&mut ring, "write through a read-only handle", |batch| {
        batch
            .write_owned(&reader, filled(0xFF), 0, 0, none, WriteCaching::Cached)
            .expect("queue");
    });
    steps.push(step);
    let (step, _) = drive(&mut ring, "read at an unaligned offset", |batch| {
        batch
            .read_owned(&file, filled(0), 0, 1, none)
            .expect("queue");
    });
    steps.push(step);

    // Four reads in one batch, and the order they complete in.
    {
        let mut batch = Batch::new(&mut ring);
        for index in 0..4_usize {
            let offset = if index % 2 == 0 { 0 } else { SECTOR as u64 };
            batch
                .read_owned(&file, filled(0), index, offset, none)
                .expect("queue");
        }
        batch.submit().expect("submit");
    }
    let mut order = Vec::new();
    let mut sizes = Vec::new();
    for _ in 0..4 {
        let (completion, held) = ring
            .pop_within(BOUND)
            .expect("pop_within")
            .expect("a completion");
        sizes.push(outcome(&completion));
        let (_buffer, index) = held.expect("held");
        order.push(index);
    }
    assert!(sizes.iter().all(|size| *size == Outcome::Done(SECTOR)));

    // A read through a registered file.
    let mut batch = Batch::new(&mut ring);
    let pending = batch
        .register_shared_files(vec![file.clone()])
        .expect("the registration builds");
    batch.submit().expect("submit");
    let (completion, _) = ring
        .pop_within(BOUND)
        .expect("pop_within")
        .expect("a completion");
    let registered = pending
        .claim_if(&completion)
        .expect("its own completion")
        .expect("the kernel registered the handle");
    let registered = registered.get(0).expect("one file");
    let (step, buffer) = drive(&mut ring, "read through a registered file", |batch| {
        batch
            .read_owned(&registered, filled(0), 0, 0, none)
            .expect("queue");
    });
    assert_eq!(buffer.expect("a buffer").as_slice(), &[0xA5; SECTOR][..]);
    steps.push(step);

    ring.run_down().expect("the ring runs down");
    drop((file, reader));
    (steps, order)
}

const HANDLE_EOF: i32 = 0x8007_0026_u32 as i32;
const ACCESS_DENIED: i32 = 0x8007_0005_u32 as i32;
const INVALID_PARAMETER: i32 = 0x8007_0057_u32 as i32;

fn outcomes(steps: &[Step]) -> Vec<(&'static str, Outcome)> {
    steps
        .iter()
        .map(|step| (step.label, step.outcome))
        .collect()
}

/// What every step of the sequence reports, for a handle that is `unbuffered` or not. Only the
/// unaligned read differs between the two modes.
fn expected(unbuffered: bool) -> Vec<(&'static str, Outcome)> {
    let unaligned = if unbuffered {
        Outcome::Failed(INVALID_PARAMETER)
    } else {
        Outcome::Done(SECTOR)
    };
    vec![
        ("write A at 0", Outcome::Done(SECTOR)),
        ("write B at 4096", Outcome::Done(SECTOR)),
        ("flush", Outcome::Done(0)),
        ("read at 0", Outcome::Done(SECTOR)),
        ("read at 4096", Outcome::Done(SECTOR)),
        ("read at the end of the file", Outcome::Failed(HANDLE_EOF)),
        ("read across the end", Outcome::Done(SECTOR)),
        (
            "write through a read-only handle",
            Outcome::Failed(ACCESS_DENIED),
        ),
        ("read at an unaligned offset", unaligned),
        ("read through a registered file", Outcome::Done(SECTOR)),
    ]
}

fn is_a_permutation(order: &[usize]) -> bool {
    let mut sorted = order.to_vec();
    sorted.sort_unstable();
    sorted == [0, 1, 2, 3]
}

/// The ring takes a handle opened with `FILE_FLAG_OVERLAPPED` and reports, for every operation in the
/// sequence, exactly what it reports for the same handle opened without it, in each buffering mode.
/// The plain handle is the control: its completions are inline, as `windows-ioring-sys`'
/// `bounded_pop.rs` records.
#[test]
fn the_ring_reports_the_same_results_on_an_overlapped_handle() {
    for unbuffered in [false, true] {
        let (plain, plain_order) =
            sequence(unbuffered, false, if unbuffered { "up" } else { "cp" });
        let (overlapped, overlapped_order) =
            sequence(unbuffered, true, if unbuffered { "uo" } else { "co" });

        assert_eq!(
            outcomes(&plain),
            expected(unbuffered),
            "plain, unbuffered={unbuffered}"
        );
        assert_eq!(
            outcomes(&overlapped),
            expected(unbuffered),
            "overlapped, unbuffered={unbuffered}"
        );

        // A synchronous handle completes during the submit, so its steps are all poppable at once
        // and a batch completes in the order it was queued.
        assert!(
            plain.iter().all(|step| step.already_there),
            "a plain handle completes inline, unbuffered={unbuffered}"
        );
        assert_eq!(plain_order, [0, 1, 2, 3]);

        // An overlapped handle may complete asynchronously, so only the set of completions is fixed.
        assert!(is_a_permutation(&overlapped_order), "{overlapped_order:?}");
    }
}
