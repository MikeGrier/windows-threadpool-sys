// Copyright (c) 2026 Mike Grier
//! Unit tests for an instance's durability state, without a ring.

use std::io;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};

use super::{
    Accepted, DEFAULT_LINEAGE, Due, Durability, Event, Flush, Key, Reach, Routing, Sealing, State,
    WriteEnd,
};
use crate::clock::StampClock;
use crate::diagnostics::NoDiagnostics;
use crate::diagnostics::tests::Recording;
use crate::domains::DomainId;
use crate::ids::{DioringIds, FailureId, FailureToken, InstanceId, OpId};
use crate::test_support::{ManualClock, Ticks, fault};
use crate::types::{FailureCause, IoFault, IoTargetKey, MarkingKind, SuspectWrite};

/// What a stamp is in these tests: a plain number, read from a clock the test moves by hand.
type Stamp = u64;

/// A clock these tests can give the core: its readings are [`Stamp`]s.
trait StampClocked: StampClock<Instant = Stamp> {}

impl<K: StampClock<Instant = Stamp>> StampClocked for K {}

/// The identities the core is tested under.
type Ids = DioringIds<u64, Stamp>;
// Failures, their reach, and their resolution: DI-3.2.4.
mod failures;
// Failure stamps, from the clock the core holds: WT-2.2.2.
mod stamps;
// Markings, and a failure's final record: DI-3.2.4.2.
mod markings;
// Minted lineages, ending and retiring them, and failures across them: DI-3.2.5.1.
mod lineages;
// Gates, and the cycles a push or a seal would close: DI-3.2.5.2.
mod gates;
// The consumer's durability provider: DI-3.2.6.
mod providers;
// What holds a file, for `remove_target`: DI-3.2.7.
mod files;

/// The core, answering for its default lineage: the one every test here exercises unless it mints
/// another. Every other method is the core's own, through `Deref`.
struct One<K = ManualClock>(Durability<u64, &'static str, Stamp, K>);

impl<K> Deref for One<K> {
    type Target = Durability<u64, &'static str, Stamp, K>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<K> DerefMut for One<K> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<K: StampClocked> One<K> {
    fn with_clock(clock: K) -> Self {
        Self(Durability::new(
            InstanceId::next(),
            clock,
            Arc::new(NoDiagnostics),
        ))
    }

    fn sealed_through(&self) -> Option<u64> {
        self.0
            .sealed_through(DEFAULT_LINEAGE)
            .expect("the default lineage is live")
    }

    fn durable_through(&self) -> Option<u64> {
        self.0
            .durable_through(DEFAULT_LINEAGE)
            .expect("the default lineage is live")
    }

    fn state(&self, epoch: u64) -> State {
        self.0
            .state(DEFAULT_LINEAGE, epoch)
            .expect("the default lineage is live")
    }

    fn refuses(&self, epoch: u64) -> Option<u64> {
        self.0.refuses(DEFAULT_LINEAGE, epoch)
    }

    fn is_abandoned(&self, epoch: u64) -> bool {
        self.0.is_abandoned(DEFAULT_LINEAGE, epoch)
    }

    fn seal(&mut self, through: u64) -> Sealing<u64, &'static str, Stamp> {
        self.0.seal(DEFAULT_LINEAGE, through)
    }

    fn flushed(
        &mut self,
        through: u64,
        file: IoTargetKey,
        result: io::Result<()>,
    ) -> Due<u64, &'static str, Stamp> {
        let result = result.map_err(fault);
        self.0.flushed(DEFAULT_LINEAGE, through, file, result)
    }

    /// An import confined to no lineage.
    fn import(
        &mut self,
        cause: FailureCause<Ids>,
        reach: &Reach,
    ) -> (FailureId, Due<u64, &'static str, Stamp>) {
        self.0.import(cause, reach, None)
    }
}

type Lineage = One;

const A: IoTargetKey = IoTargetKey(1);
const B: IoTargetKey = IoTargetKey(2);
const C: IoTargetKey = IoTargetKey(3);
const DEFAULT: Routing = Routing {
    default: true,
    provider: false,
};
/// A write that completed in full.
const FULL: WriteEnd = WriteEnd::Succeeded;
/// The fault a failed write here reports: `ERROR_IO_DEVICE`, as a Windows backend relays it. One
/// report, shared, so a nullifier made from it equals the expected one (`IoFault`'s equality).
static IO_DEVICE: LazyLock<IoFault> = LazyLock::new(|| fault(io::Error::from_raw_os_error(1117)));

/// A write that completed with the fault.
fn failed() -> WriteEnd {
    WriteEnd::Failed(IO_DEVICE.clone())
}

/// The nullifier a write that completed with the fault makes.
fn nullified() -> MarkingKind {
    MarkingKind::Nullified {
        fault: IO_DEVICE.clone(),
    }
}
/// A covered marking for a file only the built-in default answers for, as every file here is
/// unless a test routes it otherwise: the flush was observed, and no provider was asked.
const COVERED: MarkingKind = MarkingKind::Covered {
    flushed: true,
    provider: false,
};

fn lineage() -> Lineage {
    One::with_clock(ManualClock::default())
}

fn target(file: IoTargetKey) -> &'static str {
    match file {
        A => "a",
        B => "b",
        _ => "c",
    }
}

struct Ops {
    instance: InstanceId,
    next: u64,
}

impl Ops {
    fn new() -> Self {
        Self {
            instance: InstanceId::next(),
            next: 0,
        }
    }

    fn push<K: StampClocked>(
        &mut self,
        lineage: &mut One<K>,
        epoch: u64,
        file: IoTargetKey,
        routing: Routing,
    ) -> OpId {
        self.push_in(lineage, epoch, file, routing, &[])
    }

    /// Push a write to a file declared with `domains`.
    fn push_in<K: StampClocked>(
        &mut self,
        lineage: &mut One<K>,
        epoch: u64,
        file: IoTargetKey,
        routing: Routing,
        domains: &[u32],
    ) -> OpId {
        self.push_to(lineage, DEFAULT_LINEAGE, epoch, file, routing, domains)
    }

    /// Push a write to lineage `key`, of a file declared with `domains`.
    fn push_to<K: StampClocked>(
        &mut self,
        lineage: &mut One<K>,
        key: Key,
        epoch: u64,
        file: IoTargetKey,
        routing: Routing,
        domains: &[u32],
    ) -> OpId {
        let op = OpId {
            instance: self.instance,
            seq: self.next,
        };
        self.next += 1;
        let domains: Arc<[DomainId]> = domains.iter().map(|&d| DomainId(d)).collect();
        lineage.pushed(Accepted {
            op,
            lineage: key,
            epoch,
            file,
            target: target(file),
            routing,
            domains,
        });
        op
    }
}

fn flush(through: u64, file: IoTargetKey) -> Flush<u64, &'static str> {
    flush_of(DEFAULT_LINEAGE, through, file)
}

fn flush_of(lineage: Key, through: u64, file: IoTargetKey) -> Flush<u64, &'static str> {
    Flush {
        lineage,
        through,
        file,
        target: target(file),
    }
}

/// An entry a change made due, in a form a test can compare: failures by sequence number and
/// suspect sets by the sequence numbers of their writes.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    /// The default lineage's.
    Durable(u64),
    /// The default lineage's: the seal, and the failure.
    Blocked(u64, u64),
    /// Another lineage's: the lineage, and the seal.
    DurableIn(Key, u64),
    /// Another lineage's: the lineage, the seal, and the failure.
    BlockedIn(Key, u64, u64),
    /// The lineage, and the highest epoch abandoned.
    LineageEnded(Key, Option<u64>),
    Failed(u64, Vec<u64>),
    /// Failure, suspect writes, and each marking's write and kind.
    Abandoned(u64, Vec<u64>, Vec<(u64, MarkingKind)>),
    Healed(u64, Vec<u64>, Vec<(u64, MarkingKind)>),
    /// Failure, the write marked, and how.
    Marked(u64, u64, MarkingKind),
    /// The operation never issued, and the lineage and epoch named abandoned.
    NeverIssued(u64, Key, u64),
}

fn seen(due: &Due<u64, &'static str, Stamp>) -> Vec<Seen> {
    let ops =
        |writes: &[SuspectWrite<Ids>]| -> Vec<u64> { writes.iter().map(|w| w.op.seq).collect() };
    due.events
        .iter()
        .map(|event| match event {
            Event::Durable {
                lineage: DEFAULT_LINEAGE,
                through,
            } => Seen::Durable(*through),
            Event::Durable { lineage, through } => Seen::DurableIn(*lineage, *through),
            Event::Blocked {
                lineage: DEFAULT_LINEAGE,
                through,
                by,
            } => Seen::Blocked(*through, by.seq),
            Event::Blocked {
                lineage,
                through,
                by,
            } => Seen::BlockedIn(*lineage, *through, by.seq),
            Event::LineageEnded {
                lineage,
                abandoned_through,
            } => Seen::LineageEnded(*lineage, *abandoned_through),
            Event::Failed(failed) => Seen::Failed(failed.id.seq, ops(failed.suspect.writes())),
            Event::Abandoned {
                failure,
                suspect,
                markings,
            } => Seen::Abandoned(failure.seq, ops(suspect.writes()), marks(markings)),
            Event::Healed {
                failure,
                suspect,
                markings,
            } => Seen::Healed(failure.seq, ops(suspect.writes()), marks(markings)),
            Event::Marked { failure, marking } => {
                Seen::Marked(failure.seq, marking.write.seq, marking.kind.clone())
            }
            Event::NeverIssued { op, abandoned } => {
                Seen::NeverIssued(op.seq, abandoned.lineage, abandoned.epoch)
            }
        })
        .collect()
}

/// Markings by their write's sequence number and kind.
fn marks(markings: &[crate::types::Marking<Ids>]) -> Vec<(u64, MarkingKind)> {
    markings
        .iter()
        .map(|m| (m.write.seq, m.kind.clone()))
        .collect()
}

fn durable(due: &Due<u64, &'static str, Stamp>) -> Vec<u64> {
    seen(due)
        .into_iter()
        .filter_map(|s| match s {
            Seen::Durable(through) => Some(through),
            _ => None,
        })
        .collect()
}

/// The tokens of every failure a change observed.
fn tokens(due: Due<u64, &'static str, Stamp>) -> Vec<FailureToken> {
    due.events
        .into_iter()
        .filter_map(|event| match event {
            Event::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .collect()
}

/// Run `change`, which must find the core's state inconsistent: it panics where it finds it, in
/// every build, naming `what` (DI-D-46).
fn inconsistent<R>(what: &str, change: impl FnOnce() -> R) {
    let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(change)) else {
        panic!("expected a panic naming {what:?}");
    };
    let text = payload
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        text.contains(what) && text.contains("DI-D-46"),
        "expected {what:?}, got {text:?}"
    );
}

/// An inconsistency is reported to the instance's diagnostic sink before the panic, naming the
/// instance, what was inconsistent and what was in hand (DI-D-51).
#[test]
fn an_inconsistency_reaches_the_sink_before_the_panic() {
    let sink = Recording::wanting();
    let instance = InstanceId::next();
    let mut core: Durability<u64, &'static str, Stamp, ManualClock> =
        Durability::new(instance, ManualClock::default(), sink.clone());
    let never = OpId { instance, seq: 99 };
    inconsistent(
        "a write completed that was never recorded as pushed",
        || core.completed(never, WriteEnd::Succeeded),
    );
    let seen = sink.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0].starts_with(&format!(
            "instance {} found its own state inconsistent: a write completed that was never \
             recorded as pushed",
            instance.number()
        )),
        "{seen:?}"
    );
}

/// A sink that does not want inconsistencies is never given one: the core asks before it builds
/// the event (DI-D-51). The panic is the same either way.
#[test]
fn an_inconsistency_is_not_given_to_a_sink_that_does_not_want_it() {
    let sink = std::sync::Arc::new(Recording::default());
    let instance = InstanceId::next();
    let mut core: Durability<u64, &'static str, Stamp, ManualClock> =
        Durability::new(instance, ManualClock::default(), sink.clone());
    let never = OpId { instance, seq: 99 };
    inconsistent(
        "a write completed that was never recorded as pushed",
        || core.completed(never, WriteEnd::Succeeded),
    );
    assert!(sink.seen().is_empty());
}

/// The core reads its clock once for each failure it stamps, not once for each place it reports
/// the stamp: a clock that counts its readings shows how many it was asked for.
#[test]
fn the_core_reads_its_clock_once_for_each_failure_it_stamps() {
    use crate::types::ImportScope;

    let mut core: Durability<u64, &'static str, Stamp, Ticks> = Durability::new(
        InstanceId::next(),
        Ticks::default(),
        Arc::new(NoDiagnostics),
    );
    for _ in 0..2 {
        let cause = FailureCause::Imported {
            scope: ImportScope::All,
        };
        core.import(cause, &Reach::All, None);
    }
    let stamps: Vec<u64> = core.failures().iter().map(|f| f.observed).collect();
    assert_eq!(stamps, [0, 1]);
}

fn quiet(due: &Due<u64, &'static str, Stamp>) -> bool {
    due.flushes.is_empty() && due.events.is_empty()
}

fn submitted(sealing: Sealing<u64, &'static str, Stamp>) -> Due<u64, &'static str, Stamp> {
    match sealing {
        Sealing::Submitted(due) => due,
        Sealing::AlreadySealed(state) => panic!("expected a new seal, got {state:?}"),
        Sealing::Cycle(cycle) => panic!("expected a new seal, got a cycle {cycle:?}"),
    }
}

fn already(sealing: Sealing<u64, &'static str, Stamp>) -> State {
    match sealing {
        Sealing::AlreadySealed(state) => state,
        Sealing::Submitted(due) => panic!("expected a request below the seal, got {due:?}"),
        Sealing::Cycle(cycle) => panic!("expected a request below the seal, got {cycle:?}"),
    }
}

#[test]
fn a_new_lineage_has_sealed_nothing_and_every_epoch_is_open() {
    let lineage = lineage();
    assert_eq!(lineage.sealed_through(), None);
    assert_eq!(lineage.durable_through(), None);
    assert_eq!(lineage.state(0), State::Open);
    assert_eq!(lineage.refuses(0), None);
}

#[test]
fn an_inconsistency_panics_where_it_is_found_naming_what_was_in_hand() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    assert!(
        quiet(&lineage.completed(write, FULL)),
        "an ordinary completion"
    );
    inconsistent("a write completed twice", || lineage.completed(write, FULL));
    inconsistent(&format!("write {write:?} to {A:?}"), || {
        lineage.completed(write, FULL)
    });

    let mut lineage = self::lineage();
    inconsistent("a flush completed for a seal that does not exist", || {
        lineage.flushed(1, A, Ok(()))
    });
}

#[test]
fn an_inconsistency_found_while_unwinding_does_not_panic_again() {
    struct Completes<'a>(&'a mut Lineage, crate::ids::OpId);
    impl Drop for Completes<'_> {
        fn drop(&mut self) {
            self.0.completed(self.1, FULL);
        }
    }
    let mut lineage = lineage();
    let write = Ops::new().push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, FULL);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _second = Completes(&mut lineage, write);
        panic!("the test's own panic");
    }));
    let payload = unwound.expect_err("the test panicked");
    assert_eq!(
        payload.downcast_ref::<&str>().copied(),
        Some("the test's own panic"),
        "the panic in flight is the one reported, and the process did not abort"
    );
}

#[test]
fn a_seal_with_nothing_to_flush_is_durable_at_once() {
    let mut lineage = lineage();
    let due = submitted(lineage.seal(3));
    assert!(due.flushes.is_empty());
    assert_eq!(seen(&due), [Seen::Durable(3)]);
    assert_eq!(lineage.sealed_through(), Some(3));
    assert_eq!(lineage.durable_through(), Some(3));
    assert_eq!(lineage.state(3), State::Durable);
    assert_eq!(lineage.state(1), State::Durable);
    assert_eq!(lineage.state(4), State::Open);
}

#[test]
fn a_seal_waits_for_its_writes_then_flushes_their_file() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    assert!(
        quiet(&submitted(lineage.seal(2))),
        "the write is still in flight"
    );
    assert_eq!(lineage.state(1), State::Pending);

    let due = lineage.completed(write, FULL);
    assert_eq!(due.flushes, [flush(2, A)]);
    assert!(due.events.is_empty(), "the flush has not answered");

    assert_eq!(durable(&lineage.flushed(2, A, Ok(()))), [2]);
    assert_eq!(lineage.state(1), State::Durable);
}

#[test]
fn writes_above_the_seal_do_not_hold_it() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    ops.push(&mut lineage, 5, A, DEFAULT);
    assert_eq!(durable(&submitted(lineage.seal(3))), [3]);
}

#[test]
fn a_seal_flushes_each_file_once_and_waits_for_every_flush() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 1, A, DEFAULT),
        ops.push(&mut lineage, 1, B, DEFAULT),
        ops.push(&mut lineage, 2, A, DEFAULT),
    ];
    for write in writes {
        assert!(quiet(&lineage.completed(write, FULL)));
    }
    let due = submitted(lineage.seal(2));
    assert_eq!(due.flushes, [flush(2, A), flush(2, B)]);
    assert!(quiet(&lineage.flushed(2, B, Ok(()))), "A has not answered");
    assert_eq!(durable(&lineage.flushed(2, A, Ok(()))), [2]);
}

#[test]
fn a_failed_write_is_named_to_no_flush() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    lineage.completed(write, failed());
    let due = submitted(lineage.seal(1));
    assert!(due.flushes.is_empty());
    assert_eq!(seen(&due), [Seen::Durable(1)]);
}

#[test]
fn a_write_at_or_below_the_seal_is_refused_and_one_above_is_not() {
    let mut lineage = lineage();
    submitted(lineage.seal(3));
    assert_eq!(lineage.refuses(2), Some(3));
    assert_eq!(lineage.refuses(3), Some(3));
    assert_eq!(lineage.refuses(4), None);
}

#[test]
fn asking_again_reports_the_state_and_changes_nothing() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let write = ops.push(&mut lineage, 1, A, DEFAULT);
    submitted(lineage.seal(3));
    assert_eq!(already(lineage.seal(2)), State::Pending);
    assert_eq!(already(lineage.seal(3)), State::Pending);
    assert_eq!(
        lineage.sealed_through(),
        Some(3),
        "a request below does not lower the seal"
    );

    lineage.completed(write, FULL);
    lineage.flushed(3, A, Ok(()));
    assert_eq!(already(lineage.seal(1)), State::Durable);
}

#[test]
fn a_later_seal_done_first_waits_for_the_earlier_one() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let first = ops.push(&mut lineage, 1, A, DEFAULT);
    let second = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(first, FULL);
    lineage.completed(second, FULL);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert_eq!(submitted(lineage.seal(2)).flushes, [flush(2, B)]);

    assert!(
        quiet(&lineage.flushed(2, B, Ok(()))),
        "1 is not yet durable, and nothing failed, so 2 is not Blocked"
    );
    assert_eq!(lineage.state(2), State::Pending);
    assert_eq!(
        durable(&lineage.flushed(1, A, Ok(()))),
        [1, 2],
        "both, in order"
    );
    assert_eq!(lineage.durable_through(), Some(2));
}

#[test]
fn each_write_is_named_by_the_first_seal_at_or_above_it_and_by_no_other() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let low = ops.push(&mut lineage, 1, A, DEFAULT);
    let high = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(low, FULL);
    lineage.completed(high, FULL);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    lineage.flushed(1, A, Ok(()));
    assert_eq!(
        submitted(lineage.seal(2)).flushes,
        [flush(2, B)],
        "A's write was already named by the seal through 1"
    );
}

#[test]
fn a_seal_names_completed_writes_by_epoch_not_by_push_order() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let writes = [
        ops.push(&mut lineage, 5, A, DEFAULT),
        ops.push(&mut lineage, 3, B, DEFAULT),
        ops.push(&mut lineage, 4, A, DEFAULT),
    ];
    for write in writes {
        lineage.completed(write, FULL);
    }
    assert_eq!(
        submitted(lineage.seal(4)).flushes,
        [flush(4, B), flush(4, A)]
    );
    lineage.flushed(4, A, Ok(()));
    lineage.flushed(4, B, Ok(()));
    assert_eq!(submitted(lineage.seal(5)).flushes, [flush(5, A)]);
}

#[test]
fn a_seal_waits_for_a_lower_write_in_flight_while_a_higher_one_has_completed() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let lower = ops.push(&mut lineage, 1, A, DEFAULT);
    let higher = ops.push(&mut lineage, 2, B, DEFAULT);
    lineage.completed(higher, FULL);
    assert!(quiet(&submitted(lineage.seal(2))));
    let due = lineage.completed(lower, FULL);
    assert_eq!(due.flushes, [flush(2, A), flush(2, B)]);
}

#[test]
fn a_write_a_provider_must_answer_for_leaves_its_seal_pending() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let only = Routing {
        default: false,
        provider: true,
    };
    lineage.serve(Arc::from([DomainId(1)]));
    let write = ops.push_in(&mut lineage, 1, A, only, &[1]);
    lineage.completed(write, FULL);
    assert!(
        quiet(&submitted(lineage.seal(1))),
        "no default flush, and the provider has not answered"
    );
    assert_eq!(lineage.state(1), State::Pending);
}

#[test]
fn a_file_both_answer_for_is_flushed_and_still_waits_for_the_provider() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    let both = Routing {
        default: true,
        provider: true,
    };
    lineage.serve(Arc::from([DomainId(1)]));
    let write = ops.push_in(&mut lineage, 1, A, both, &[1, 2]);
    lineage.completed(write, FULL);
    assert_eq!(submitted(lineage.seal(1)).flushes, [flush(1, A)]);
    assert!(quiet(&lineage.flushed(1, A, Ok(()))));
    assert_eq!(lineage.state(1), State::Pending);
}
