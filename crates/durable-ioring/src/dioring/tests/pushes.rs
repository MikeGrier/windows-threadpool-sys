// Copyright (c) 2026 Mike Grier
//! Pushes, their completions, and the readiness signal (DI-3.2.2.1), against real files.
//!
//! Every push and every entry is reported to the conformance oracle, so each test also checks
//! one completion per operation with its context handed back, without restating that rule.
//! Waits poll the queue under a bound; the readiness signal has its own check.

use std::fs;
use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use win_shared_os_owned_handle::SharedHandle;
use win_time_sys::InterruptClock;
use windows_ioring_sys::RegisteredSpan;

use super::{TempFile, V, empty, setup};
use crate::contract::{DurableRing, RegisteredBufferRing};
use crate::dioring::{Dioring, FileSetup, Setup, TimeBase};
use crate::ids::OpId;
use crate::oracle::{ConformanceOracle, check_readiness};
use crate::types::{
    DurabilityRequest, Entry, Epoch, FileKey, FileOptions, OpCompletion, OpKind, Outcome,
    PushRefusal, WriteCaching, WriteOptions,
};

/// The instances here carry a `u32` context, so the oracle can compare them, and stamp failures
/// with `K`.
pub(super) type Ring<K = InterruptClock> = Dioring<Vec<u8>, u64, u32, Vec<u8>, K>;
pub(super) type Completed = OpCompletion<V, Vec<u8>, u32>;
pub(super) type RingEntry = Entry<V, Vec<u8>, u32>;

/// How long a test waits for a completion before calling it lost.
pub(super) const BOUND: Duration = Duration::from_secs(5);
/// Large enough for the most operations any test here has in flight at once.
const QUEUE: u32 = 128;

pub(super) const ADDED: FileKey = FileKey(1);
pub(super) const GIVEN: FileKey = FileKey(2);

pub(super) fn temp(content: &[u8]) -> TempFile {
    let temp = TempFile::new("io");
    fs::write(&temp.0, content).expect("fill the temporary file");
    temp
}

pub(super) fn read_only(temp: &TempFile) -> SharedHandle {
    let file = fs::OpenOptions::new()
        .read(true)
        .open(&temp.0)
        .expect("open read-only");
    SharedHandle::new(OwnedHandle::from(file))
}

pub(super) fn instance(files: Vec<FileSetup>, buffers: Vec<Vec<u8>>) -> Ring {
    instance_with_clock(files, buffers, InterruptClock)
}

/// An instance stamping failures with `clock`.
pub(super) fn instance_with_clock<K: TimeBase>(
    files: Vec<FileSetup>,
    buffers: Vec<Vec<u8>>,
    clock: K,
) -> Ring<K> {
    Ring::with_clock(
        Setup {
            submission_queue_size: QUEUE,
            completion_queue_size: QUEUE,
            ..setup(files, buffers, None)
        },
        clock,
    )
    .expect("build an instance")
}

pub(super) fn given(key: FileKey, temp: &TempFile) -> FileSetup {
    FileSetup {
        key,
        file: temp.open(),
        options: FileOptions::new(),
    }
}

/// An instance and the oracle watching its stream.
pub(super) struct Harness<K = InterruptClock> {
    pub(super) ring: Ring<K>,
    pub(super) oracle: ConformanceOracle<V, u32>,
}

impl<K: TimeBase> Harness<K> {
    pub(super) fn new(ring: Ring<K>) -> Self {
        Self {
            ring,
            oracle: ConformanceOracle::new(),
        }
    }

    pub(super) fn epoch(&self, id: u64) -> Epoch<V> {
        Epoch::new(self.ring.default_lineage(), id)
    }

    pub(super) fn write(
        &mut self,
        file: FileKey,
        offset: u64,
        bytes: &[u8],
        epoch: u64,
        context: u32,
    ) -> OpId {
        let epoch = self.epoch(epoch);
        let id = self
            .ring
            .write(file, offset, bytes.to_vec(), epoch, context)
            .expect("push a write");
        self.pushed(id, OpKind::Write { epoch }, context);
        id
    }

    pub(super) fn read(&mut self, file: FileKey, offset: u64, len: usize, context: u32) -> OpId {
        let id = self
            .ring
            .read(file, offset, vec![0; len], context)
            .expect("push a read");
        self.pushed(id, OpKind::Read, context);
        id
    }

    pub(super) fn pushed(&mut self, id: OpId, kind: OpKind<V>, context: u32) {
        self.oracle
            .pushed(id, kind, context)
            .expect("the oracle accepts the push");
    }

    /// The next entry, within the bound, after the oracle has accepted it.
    pub(super) fn next_entry(&mut self) -> RingEntry {
        let started = Instant::now();
        loop {
            if let Some(entry) = self.ring.pop().expect("pop") {
                self.oracle
                    .observe(&entry)
                    .expect("the oracle accepts the entry");
                return entry;
            }
            assert!(
                started.elapsed() < BOUND,
                "no entry within {BOUND:?}; {} outstanding",
                self.oracle.outstanding()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The next entry, which must be a completion.
    pub(super) fn next(&mut self) -> Completed {
        match self.next_entry() {
            Entry::Op(completion) => completion,
            other => panic!("expected a completion, got {other:?}"),
        }
    }

    /// The next `n` completions, in the order they were popped.
    pub(super) fn next_n(&mut self, n: usize) -> Vec<Completed> {
        (0..n).map(|_| self.next()).collect()
    }

    /// Ask for durability through `id` in the default lineage, reporting a new seal to the
    /// oracle.
    pub(super) fn seal(&mut self, id: u64) -> DurabilityRequest<V> {
        let through = self.epoch(id);
        let answer = self
            .ring
            .make_durable_through(through)
            .expect("seal the default lineage");
        if answer == DurabilityRequest::Submitted {
            self.oracle.sealed(through);
        }
        answer
    }

    pub(super) fn finish(self) {
        self.oracle.finish().expect("every operation completed");
    }
}

pub(super) fn transferred(completion: &Completed) -> u32 {
    match completion.outcome {
        Outcome::Transferred(n) => n,
        ref other => panic!("expected a transfer, got {other:?}"),
    }
}

/// Write `bytes` and read them back through `file`, checking both completions.
fn round_trip(harness: &mut Harness, file: FileKey) {
    let bytes = b"durable ioring round trip";
    let write = harness.write(file, 3, bytes, 1, 10);
    let written = harness.next();
    assert_eq!(written.id, write);
    assert_eq!(transferred(&written), bytes.len() as u32);
    assert_eq!(
        written.buffer.as_deref(),
        Some(&bytes[..]),
        "the buffer comes back"
    );
    assert_eq!(written.context, 10);

    let read = harness.read(file, 3, bytes.len(), 11);
    let back = harness.next();
    assert_eq!(back.id, read);
    assert_eq!(transferred(&back), bytes.len() as u32);
    assert_eq!(
        back.buffer.as_deref(),
        Some(&bytes[..]),
        "the read filled its buffer"
    );
    assert_eq!(back.context, 11);
}

#[test]
fn a_write_and_a_read_round_trip_through_a_file_added_later() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    harness
        .ring
        .add_file(ADDED, temp.open())
        .expect("add the file");
    round_trip(&mut harness, ADDED);
    harness.finish();
}

#[test]
fn a_write_and_a_read_round_trip_through_a_file_given_at_construction() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    round_trip(&mut harness, GIVEN);
    harness.finish();
}

#[test]
fn spans_of_the_registered_buffers_round_trip() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], vec![vec![0; 32]; 2]));
    harness
        .ring
        .registered_buffer_mut(0)
        .expect("fill buffer 0")[..16]
        .copy_from_slice(b"registered bytes");

    let epoch = harness.epoch(1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 16,
    };
    let write = harness
        .ring
        .write_registered(GIVEN, 8, span, epoch, 1)
        .expect("push a registered write");
    harness.pushed(write, OpKind::Write { epoch }, 1);
    let written = harness.next();
    assert_eq!(transferred(&written), 16);
    assert!(
        written.buffer.is_none(),
        "a registered span has no buffer to return"
    );

    let into = RegisteredSpan {
        buffer_index: 1,
        offset: 4,
        len: 16,
    };
    let read = harness
        .ring
        .read_registered(GIVEN, 8, into, 2)
        .expect("push a registered read");
    harness.pushed(read, OpKind::Read, 2);
    let back = harness.next();
    assert_eq!(transferred(&back), 16);
    assert!(back.buffer.is_none());
    assert_eq!(
        &harness.ring.registered_buffer(1).expect("read buffer 1")[4..20],
        b"registered bytes"
    );
    assert_eq!(
        fs::read(&temp.0).expect("read the file")[8..24],
        *b"registered bytes"
    );
    harness.finish();
}

#[test]
fn every_operation_completes_once_with_its_context() {
    let first = temp(&[0; 1024]);
    let second = temp(&[0; 1024]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &first)], Vec::new()));
    harness
        .ring
        .add_file(ADDED, second.open())
        .expect("add a file");

    for i in 0..32_u32 {
        let file = if i % 2 == 0 { GIVEN } else { ADDED };
        harness.write(file, u64::from(i) * 16, &[i as u8; 16], 1, i);
    }
    let mut contexts: Vec<u32> = harness.next_n(32).iter().map(|c| c.context).collect();
    contexts.sort_unstable();
    assert_eq!(contexts, (0..32).collect::<Vec<_>>());

    for i in 0..32_u32 {
        let file = if i % 2 == 0 { GIVEN } else { ADDED };
        harness.read(file, u64::from(i) * 16, 16, 100 + i);
    }
    for completion in harness.next_n(32) {
        let i = completion.context - 100;
        assert_eq!(
            completion.buffer,
            Some(vec![i as u8; 16]),
            "read {i} found its write"
        );
    }
    harness.finish();
}

#[test]
fn operation_identities_follow_push_order() {
    let temp = temp(&[0; 64]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let ids: Vec<OpId> = (0..5).map(|i| harness.read(GIVEN, 0, 4, i)).collect();
    for pair in ids.windows(2) {
        assert!(
            pair[0] < pair[1],
            "{:?} was pushed before {:?}",
            pair[0],
            pair[1]
        );
    }
    let other = empty().default_lineage();
    assert_ne!(other, harness.ring.default_lineage());
    harness.next_n(5);
    harness.finish();
}

#[test]
fn a_write_the_kernel_fails_completes_as_failed() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    harness
        .ring
        .add_file(ADDED, read_only(&temp))
        .expect("add a read-only file");
    harness.write(ADDED, 0, b"refused", 1, 5);
    let completion = harness.next();
    assert!(
        matches!(completion.outcome, Outcome::Failed(_)),
        "a write through a read-only handle fails: {:?}",
        completion.outcome
    );
    assert_eq!(
        completion.buffer.as_deref(),
        Some(&b"refused"[..]),
        "the buffer still comes back"
    );
    assert_eq!(completion.context, 5);
    harness.finish();
}

#[test]
fn a_read_past_the_end_reports_what_was_transferred() {
    let temp = temp(b"ten bytes!");
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    harness.read(GIVEN, 0, 16, 1);
    let completion = harness.next();
    assert_eq!(
        transferred(&completion),
        10,
        "a short transfer is reported as it is"
    );
    assert_eq!(&completion.buffer.expect("the buffer")[..10], b"ten bytes!");
    harness.finish();
}

/// The caching choice reaches the kernel: measured on 2026-10-07, `IoRing`'s write-through flag
/// fails with error 509 ("not supported on a file opened for cached IO") on a cached handle --
/// including one opened with `FILE_FLAG_WRITE_THROUGH` -- and succeeds only on an unbuffered
/// one. So the same write completes as a transfer when cached and as a failure when written
/// through, which a dropped flag could not produce. The failure is still a completion, with its
/// buffer and context.
#[test]
fn the_caching_choice_reaches_the_kernel() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let epoch = harness.epoch(1);
    for (caching, context) in [(WriteCaching::Cached, 3), (WriteCaching::WriteThrough, 4)] {
        let id = harness
            .ring
            .write_with(
                GIVEN,
                0,
                b"through".to_vec(),
                epoch,
                context,
                WriteOptions::new().caching(caching),
            )
            .expect("push a write");
        harness.pushed(id, OpKind::Write { epoch }, context);
    }
    let mut completions = harness.next_n(2);
    completions.sort_by_key(|c| c.context);
    assert_eq!(transferred(&completions[0]), 7, "a cached write");
    assert!(
        matches!(completions[1].outcome, Outcome::Failed(_)),
        "a write-through write on a cached handle: {:?}",
        completions[1].outcome
    );
    assert_eq!(completions[1].buffer.as_deref(), Some(&b"through"[..]));
    harness.finish();
}

#[test]
fn pushes_naming_a_file_the_instance_was_not_given_are_refused_with_what_they_took() {
    let mut ring = instance(Vec::new(), vec![vec![0; 16]]);
    let epoch = Epoch::new(ring.default_lineage(), 1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let unknown = FileKey(99);

    let error = ring
        .write(unknown, 0, vec![1, 2], epoch, 1)
        .expect_err("a write");
    assert!(matches!(error.reason, PushRefusal::UnknownFile(key) if key == unknown));
    assert_eq!((error.buffer, error.context), (Some(vec![1, 2]), 1));

    let error = ring.read(unknown, 0, vec![3], 2).expect_err("a read");
    assert!(matches!(error.reason, PushRefusal::UnknownFile(key) if key == unknown));
    assert_eq!((error.buffer, error.context), (Some(vec![3]), 2));

    let error = ring
        .write_registered(unknown, 0, span, epoch, 3)
        .expect_err("a registered write");
    assert!(matches!(error.reason, PushRefusal::UnknownFile(key) if key == unknown));
    assert_eq!((error.buffer, error.context), (None, 3));

    let error = ring
        .read_registered(unknown, 0, span, 4)
        .expect_err("a registered read");
    assert!(matches!(error.reason, PushRefusal::UnknownFile(key) if key == unknown));
    assert_eq!((error.buffer, error.context), (None, 4));
    assert!(
        ring.pop().expect("pop").is_none(),
        "a refused push completes nothing"
    );
}

#[test]
fn a_write_tagged_with_another_instances_lineage_is_refused() {
    let temp = temp(&[0; 16]);
    let mut ring = instance(vec![given(GIVEN, &temp)], vec![vec![0; 16]]);
    let foreign = empty().default_lineage();
    let epoch = Epoch::new(foreign, 1);

    let error = ring
        .write(GIVEN, 0, vec![1], epoch, 1)
        .expect_err("a write");
    assert!(matches!(error.reason, PushRefusal::UnknownLineage(l) if l == foreign));
    assert_eq!((error.buffer, error.context), (Some(vec![1]), 1));

    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let error = ring
        .write_registered(GIVEN, 0, span, epoch, 2)
        .expect_err("a registered write");
    assert!(matches!(error.reason, PushRefusal::UnknownLineage(l) if l == foreign));
    assert_eq!((error.buffer, error.context), (None, 2));
}

#[test]
fn span_operations_on_an_instance_without_registered_buffers_are_refused() {
    let temp = temp(&[0; 16]);
    let mut ring = instance(vec![given(GIVEN, &temp)], Vec::new());
    let epoch = Epoch::new(ring.default_lineage(), 1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let error = ring
        .write_registered(GIVEN, 0, span, epoch, 1)
        .expect_err("a registered write");
    assert!(matches!(error.reason, PushRefusal::NoRegisteredBuffers));
    assert_eq!((error.buffer, error.context), (None, 1));

    let error = ring
        .read_registered(GIVEN, 0, span, 2)
        .expect_err("a registered read");
    assert!(matches!(error.reason, PushRefusal::NoRegisteredBuffers));
    assert_eq!((error.buffer, error.context), (None, 2));

    let refused = ring
        .registered_buffer(0)
        .expect_err("no registration to read");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    let refused = ring
        .registered_buffer_mut(0)
        .expect_err("no registration to fill");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn a_span_the_registration_does_not_contain_is_refused_by_the_ring() {
    let temp = temp(&[0; 16]);
    let mut ring = instance(vec![given(GIVEN, &temp)], vec![vec![0; 16]]);
    let epoch = Epoch::new(ring.default_lineage(), 1);
    let outside = RegisteredSpan {
        buffer_index: 0,
        offset: 8,
        len: 16,
    };
    let error = ring
        .write_registered(GIVEN, 0, outside, epoch, 7)
        .expect_err("a span past the buffer's end");
    assert!(matches!(error.reason, PushRefusal::Ring(_)));
    assert_eq!((error.buffer, error.context), (None, 7));

    let missing = RegisteredSpan {
        buffer_index: 1,
        offset: 0,
        len: 4,
    };
    let error = ring
        .read_registered(GIVEN, 0, missing, 8)
        .expect_err("a buffer the registration does not have");
    assert!(matches!(error.reason, PushRefusal::Ring(_)));
    assert_eq!((error.buffer, error.context), (None, 8));

    assert_eq!(
        ring.registered_buffer(1).expect_err("out of range").kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert!(
        ring.pop().expect("pop").is_none(),
        "a refused push completes nothing"
    );
}

#[test]
fn a_refused_push_spends_no_identity() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let before = harness.read(GIVEN, 0, 4, 1);
    harness
        .ring
        .read(FileKey(99), 0, vec![0; 4], 2)
        .expect_err("an unknown file");
    let after = harness.read(GIVEN, 0, 4, 3);
    assert_eq!(
        after.seq,
        before.seq + 1,
        "identities of accepted pushes are consecutive"
    );
    harness.next_n(2);
    harness.finish();
}

#[test]
fn a_file_added_under_a_key_in_use_is_refused_and_handed_back() {
    let temp = temp(&[0; 16]);
    let mut ring = instance(vec![given(GIVEN, &temp)], Vec::new());
    ring.add_file(ADDED, temp.open()).expect("a new key");

    for key in [GIVEN, ADDED] {
        let file = temp.open();
        let raw = file.as_raw_handle();
        let error = ring
            .add_file_with(key, file, FileOptions::new().domains([super::domain("d")]))
            .expect_err("a key already in use");
        assert_eq!(error.key, key);
        assert_eq!(
            error.file.as_raw_handle(),
            raw,
            "the consumer's own handle comes back"
        );
        assert_eq!(error.options.domains, vec![super::domain("d")]);
    }
    assert!(
        !ring.domains.contains_key(&super::domain("d")),
        "a refused file's domains are not interned"
    );
}

#[test]
fn an_idle_instance_has_nothing_to_pop() {
    let mut ring = instance(Vec::new(), Vec::new());
    assert!(ring.pop().expect("pop").is_none());
    assert!(ring.pop().expect("pop again").is_none());
}

#[test]
fn the_readiness_signal_is_set_on_every_empty_to_non_empty_transition() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(vec![given(GIVEN, &temp)], Vec::new()));
    let mut next = 0;
    let mut pushed = Vec::new();
    let entries = check_readiness(
        &mut harness.ring,
        |ring| {
            let id = ring.read(GIVEN, 0, vec![0; 4], next).expect("push a read");
            pushed.push((id, next));
            next += 1;
        },
        BOUND,
    )
    .expect("dioring keeps DI-D-28's rule");
    for (id, context) in pushed {
        harness.pushed(id, OpKind::Read, context);
    }
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        harness
            .oracle
            .observe(entry)
            .expect("the oracle accepts the entry");
    }
    harness.finish();
}

#[test]
fn dropping_an_instance_with_operations_in_flight_returns() {
    let temp = temp(&[0; 256]);
    let (dropped, done) = std::sync::mpsc::channel();
    let mut ring = instance(vec![given(GIVEN, &temp)], Vec::new());
    let epoch = Epoch::new(ring.default_lineage(), 1);
    for i in 0..16 {
        ring.write(GIVEN, i * 16, vec![1; 16], epoch, i as u32)
            .expect("push a write");
    }
    std::thread::spawn(move || {
        drop(ring);
        let _ = dropped.send(());
    });
    done.recv_timeout(BOUND).expect("the drop returned");
}
