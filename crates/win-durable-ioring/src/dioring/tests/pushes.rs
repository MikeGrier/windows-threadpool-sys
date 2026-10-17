// Copyright (c) 2026 Mike Grier
//! Pushes, their completions, and the readiness signal (DI-3.2.2.1), against real files.
//!
//! Every push and every entry is reported to the conformance oracle, so each test also checks
//! one completion per operation with its context handed back, without restating that rule.
//! Waits poll the queue under a bound; the readiness signal has its own check.

use std::fs;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};

use windows_ioring_sys::RegisteredSpan;

use super::{TempFile, V, empty, setup, wtf};

/// A test buffer's length, as the oracle's push hook takes it.
fn length(len: usize) -> u32 {
    u32::try_from(len).expect("a test buffer's length fits a u32")
}
use crate::InterruptStamps;
use crate::dioring::{Dioring, Setup, TargetSetup, TimeBase};
use crate::readiness::check_readiness;
use crate::windows_backend::{Cached, FileOptions, OpenedFile, ReadOnly, Win32FileTarget};
use durable_ioring::backend::RefusalCause;
use durable_ioring::oracle::ConformanceOracle;
use durable_ioring::{Access, IoTarget};
use durable_ioring::{
    DurabilityRequest, Entry, Epoch, EpochState, ImportScope, IoTargetKey, OpCompletion, OpKind,
    Outcome, PushRefusal, WriteCaching, WriteOptions,
};
use durable_ioring::{DurableRing, RegisteredBufferRing};
use durable_ioring::{Lineage, LineageHandle, OpId};

/// The instances here carry a `u32` context, so the oracle can compare them, and stamp failures
/// with `K`.
pub(super) type Ring<K = InterruptStamps> = Dioring<Vec<u8>, u64, u32, Vec<u8>, K>;
pub(super) type Completed = OpCompletion<V, Vec<u8>, u32>;
pub(super) type RingEntry = Entry<V, Vec<u8>, u32>;

/// How long a test waits for a completion before calling it lost.
pub(super) const BOUND: Duration = Duration::from_secs(5);
/// Large enough for the most operations any test here has in flight at once.
const QUEUE: u32 = 128;

pub(super) const ADDED: IoTargetKey = IoTargetKey(1);
pub(super) const GIVEN: IoTargetKey = IoTargetKey(2);

pub(super) fn temp(content: &[u8]) -> TempFile {
    let temp = TempFile::new("io");
    fs::write(&temp.0, content).expect("fill the temporary file");
    temp
}

pub(super) fn read_only(temp: &TempFile) -> OpenedFile<Cached, ReadOnly> {
    OpenedFile::open(wtf(&temp.0), FileOptions::new().read()).expect("open read-only")
}

pub(super) fn instance(files: Vec<TargetSetup>, buffers: Vec<Vec<u8>>) -> Ring {
    instance_with_clock(files, buffers, InterruptStamps)
}

/// An instance stamping failures with `clock`.
pub(super) fn instance_with_clock<K: TimeBase>(
    files: Vec<TargetSetup>,
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
    .0
}

pub(super) fn given(key: IoTargetKey, temp: &TempFile) -> TargetSetup {
    TargetSetup::new(key, Win32FileTarget::new(temp.open()))
}

/// An instance and the oracle watching its stream.
pub(super) struct Harness<K: TimeBase = InterruptStamps> {
    pub(super) ring: Ring<K>,
    pub(super) oracle: ConformanceOracle<V, u32>,
    /// A handle to the instance's default lineage, which every write here is tagged in.
    pub(super) default: LineageHandle,
}

impl<K: TimeBase> Harness<K> {
    pub(super) fn new(ring: Ring<K>) -> Self {
        Self {
            default: ring.default_lineage(),
            ring,
            oracle: ConformanceOracle::new(),
        }
    }

    /// The default lineage.
    pub(super) fn lineage(&self) -> Lineage {
        self.default.lineage()
    }

    pub(super) fn epoch(&self, id: u64) -> Epoch<V> {
        Epoch::new(self.lineage(), id)
    }

    /// The default lineage's seal point.
    pub(super) fn sealed(&self) -> Option<u64> {
        (self.ring.sealed_through(self.lineage())).expect("the default lineage is live")
    }

    /// The default lineage's high-water mark.
    pub(super) fn durable(&self) -> Option<u64> {
        (self.ring.durable_through(self.lineage())).expect("the default lineage is live")
    }

    /// The state of the default lineage's epoch `id`.
    pub(super) fn state(&self, id: u64) -> EpochState<V> {
        (self.ring.epoch_state(self.epoch(id))).expect("the default lineage is live")
    }

    pub(super) fn write(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        bytes: &[u8],
        epoch: u64,
        context: u32,
    ) -> OpId {
        let default = self.default.clone();
        self.write_in(&default, file, offset, bytes, epoch, context)
    }

    /// Write `bytes` tagged with epoch `epoch` of `lineage`'s lineage.
    pub(super) fn write_in(
        &mut self,
        lineage: &LineageHandle,
        file: IoTargetKey,
        offset: u64,
        bytes: &[u8],
        epoch: u64,
        context: u32,
    ) -> OpId {
        let id = self
            .ring
            .write(file, offset, bytes.to_vec(), lineage.at(epoch), context)
            .expect("push a write");
        let epoch = Epoch::new(lineage.lineage(), epoch);
        self.pushed(id, OpKind::Write { epoch }, length(bytes.len()), context);
        id
    }

    pub(super) fn read(
        &mut self,
        file: IoTargetKey,
        offset: u64,
        len: usize,
        context: u32,
    ) -> OpId {
        let id = self
            .ring
            .read(file, offset, vec![0; len], context)
            .expect("push a read");
        self.pushed(id, OpKind::Read, length(len), context);
        id
    }

    pub(super) fn pushed(&mut self, id: OpId, kind: OpKind<V>, length: u32, context: u32) {
        self.oracle
            .pushed(id, kind, length, context)
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
        let default = self.default.clone();
        self.seal_in(&default, id)
    }

    /// Ask for durability through `id` in `lineage`'s lineage, reporting a new seal to the
    /// oracle.
    pub(super) fn seal_in(&mut self, lineage: &LineageHandle, id: u64) -> DurabilityRequest<V> {
        let answer = self
            .ring
            .make_durable_through(lineage.at(id))
            .expect("seal a live lineage");
        if answer == DurabilityRequest::Submitted {
            self.oracle.sealed(Epoch::new(lineage.lineage(), id));
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
fn round_trip(harness: &mut Harness, file: IoTargetKey) {
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
        .add_target(ADDED, Win32FileTarget::new(temp.open()))
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
        .with_registered_buffer_mut(0, |bytes| {
            bytes[..16].copy_from_slice(b"registered bytes");
        })
        .expect("fill buffer 0");

    let epoch = harness.epoch(1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 16,
    };
    let write = harness
        .ring
        .write_registered(GIVEN, 8, span, harness.default.at(1), 1)
        .expect("push a registered write");
    harness.pushed(write, OpKind::Write { epoch }, span.len, 1);
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
    harness.pushed(read, OpKind::Read, into.len, 2);
    let back = harness.next();
    assert_eq!(transferred(&back), 16);
    assert!(back.buffer.is_none());
    assert_eq!(
        harness
            .ring
            .with_registered_buffer(1, |bytes| bytes[4..20].to_vec())
            .expect("read buffer 1"),
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
        .add_target(ADDED, Win32FileTarget::new(second.open()))
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
    let other = empty().default_lineage().lineage();
    assert_ne!(other, harness.lineage());
    harness.next_n(5);
    harness.finish();
}

#[test]
fn a_write_to_a_read_only_target_is_refused_at_the_push_with_its_buffer() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    harness
        .ring
        .add_target(ADDED, Win32FileTarget::new(read_only(&temp)))
        .expect("add a read-only file");
    let refused = harness
        .ring
        .write(ADDED, 0, b"refused".to_vec(), harness.default.at(1), 5)
        .expect_err("a write the access does not cover");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::AccessDenied)
    ));
    assert_eq!(refused.buffer.as_deref(), Some(&b"refused"[..]));
    assert_eq!(refused.context, 5);
    assert_eq!(
        fs::read(&temp.0).expect("read"),
        [0; 16],
        "nothing was written"
    );
    harness.finish();
}

#[test]
fn a_read_from_a_write_only_target_is_refused_at_the_push_and_a_write_to_it_is_not() {
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    let file = OpenedFile::<Cached, _>::open(wtf(&temp.0), FileOptions::new().write())
        .expect("open write-only");
    harness
        .ring
        .add_target(ADDED, Win32FileTarget::new(file))
        .expect("add a write-only file");
    let refused = harness
        .ring
        .read(ADDED, 0, vec![0; 4], 6)
        .expect_err("a read the access does not cover");
    assert!(matches!(
        refused.reason,
        PushRefusal::Backend(RefusalCause::AccessDenied)
    ));
    assert_eq!(refused.buffer, Some(vec![0; 4]));

    harness.write(ADDED, 0, b"taken", 1, 7);
    assert_eq!(transferred(&harness.next()), 5);
    harness.finish();
}

#[test]
fn a_write_the_kernel_fails_completes_as_failed() {
    // A handle that cannot write, stated by its target to be one that can: the core trusts what a
    // target states, so what is exercised is the kernel's own refusal reaching the completion.
    let temp = temp(&[0; 16]);
    let mut harness = Harness::new(instance(Vec::new(), Vec::new()));
    harness
        .ring
        .add_target(
            ADDED,
            Win32FileTarget::new(read_only(&temp)).claiming(Access::ReadWrite),
        )
        .expect("add a read-only file that claims to be writable");
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
                harness.default.at(1),
                context,
                WriteOptions::new().caching(caching),
            )
            .expect("push a write");
        harness.pushed(id, OpKind::Write { epoch }, 7, context);
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
    let default = ring.default_lineage();
    let epoch = default.at(1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let unknown = IoTargetKey(99);

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
    // A handle to a lineage of an instance already gone: it names a lineage, and nothing more.
    let handle = empty().default_lineage();
    let foreign = handle.lineage();
    let epoch = handle.at(1);

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
    let default = ring.default_lineage();
    let epoch = default.at(1);
    let span = RegisteredSpan {
        buffer_index: 0,
        offset: 0,
        len: 4,
    };
    let error = ring
        .write_registered(GIVEN, 0, span, epoch, 1)
        .expect_err("a registered write");
    assert!(matches!(
        error.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));
    assert_eq!((error.buffer, error.context), (None, 1));

    let error = ring
        .read_registered(GIVEN, 0, span, 2)
        .expect_err("a registered read");
    assert!(matches!(
        error.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));
    assert_eq!((error.buffer, error.context), (None, 2));

    // Gated, it would be held rather than pushed; it is refused at push all the same (DI-D-54).
    let error = ring
        .write_registered_with(
            GIVEN,
            0,
            span,
            default.at(2),
            3,
            WriteOptions::new().gate(epoch.epoch()),
        )
        .expect_err("a gated registered write");
    assert!(matches!(
        error.reason,
        PushRefusal::Backend(RefusalCause::Unsupported)
    ));
    assert_eq!((error.buffer, error.context), (None, 3));

    let refused = ring
        .with_registered_buffer(0, |_| unreachable!("nothing is lent"))
        .expect_err("no registration to read");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    let refused = ring
        .with_registered_buffer_mut(0, |_| unreachable!("nothing is lent"))
        .expect_err("no registration to fill");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn a_span_the_registration_does_not_contain_is_refused_by_the_ring() {
    let temp = temp(&[0; 16]);
    let mut ring = instance(vec![given(GIVEN, &temp)], vec![vec![0; 16]]);
    let default = ring.default_lineage();
    let epoch = default.at(1);
    let outside = RegisteredSpan {
        buffer_index: 0,
        offset: 8,
        len: 16,
    };
    let error = ring
        .write_registered(GIVEN, 0, outside, epoch, 7)
        .expect_err("a span past the buffer's end");
    assert!(matches!(
        error.reason,
        PushRefusal::Backend(RefusalCause::Invalid)
    ));
    assert_eq!((error.buffer, error.context), (None, 7));

    // Gated, it is refused at push too, the registration's own check asked first (DI-D-54).
    let error = ring
        .write_registered_with(
            GIVEN,
            0,
            outside,
            default.at(2),
            9,
            WriteOptions::new().gate(epoch.epoch()),
        )
        .expect_err("a gated span past the buffer's end");
    assert!(matches!(
        error.reason,
        PushRefusal::Backend(RefusalCause::Invalid)
    ));
    assert_eq!((error.buffer, error.context), (None, 9));

    let missing = RegisteredSpan {
        buffer_index: 1,
        offset: 0,
        len: 4,
    };
    let error = ring
        .read_registered(GIVEN, 0, missing, 8)
        .expect_err("a buffer the registration does not have");
    assert!(matches!(
        error.reason,
        PushRefusal::Backend(RefusalCause::Invalid)
    ));
    assert_eq!((error.buffer, error.context), (None, 8));

    assert_eq!(
        ring.with_registered_buffer(1, |_| unreachable!("nothing is lent"))
            .expect_err("out of range")
            .kind(),
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
        .read(IoTargetKey(99), 0, vec![0; 4], 2)
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
    ring.add_target(ADDED, Win32FileTarget::new(temp.open()))
        .expect("a new key");

    for key in [GIVEN, ADDED] {
        let file = temp.open();
        let raw = file.as_raw_handle();
        let error = ring
            .add_target(key, super::target(file, &["d"]))
            .expect_err("a key already in use");
        assert_eq!(error.key, key);
        assert_eq!(
            error.target.handle().as_raw_handle(),
            raw,
            "the consumer's own handle comes back"
        );
        assert_eq!(error.target.flush_domains(), [super::domain("d")]);
    }
    let imported = ring
        .import_failure(ImportScope::Domains(vec![super::domain("d")]))
        .expect("an import by domain");
    assert_eq!(
        imported.unmatched,
        [super::domain("d")],
        "a refused target's domains were never given to the instance"
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
        harness.pushed(id, OpKind::Read, 4, context);
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
    let default = ring.default_lineage();
    let epoch = default.at(1);
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
