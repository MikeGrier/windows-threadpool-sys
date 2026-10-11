// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider (DI-3.2.6, DI-D-27, DI-D-44), against real files: what a
//! seal asks it, its answers from inside `make_durable` and from another thread, what its failure
//! and its silence are reported as, and what a panic in it does. Every entry is reported to the
//! conformance oracle.

use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use windows_threadpool_sys::wait::ThreadpoolWait;

use super::pushes::{BOUND, GIVEN, Harness, Ring, temp};
use super::{FlushDomain, FlushRequest, Kept, TempFile, V, domain, file, keeper, setup};
use crate::backend::SubmissionData;
use crate::contract::DurableRing;
use crate::provider::DurabilityProvider;
use crate::types::{
    DurabilityRequest, Entry, EpochState, Failed, FailureCause, IoTargetKey, OpKind, WriteOptions,
};

/// `ERROR_IO_DEVICE`, the error a failing provider reports.
const IO_DEVICE: i32 = 1117;

/// An instance over `temp`, as `GIVEN` in `domains`, with `provider`.
fn instance(
    temp: &TempFile,
    domains: &[&str],
    provider: Option<Box<dyn DurabilityProvider<V>>>,
) -> Harness {
    Harness::new(
        Ring::new(setup(
            vec![file(GIVEN.0, temp, domains)],
            Vec::new(),
            provider,
        ))
        .expect("build an instance with a provider")
        .0,
    )
}

/// The requests the provider has been handed so far, taken.
fn taken(kept: &Kept) -> Vec<FlushRequest<V>> {
    std::mem::take(&mut *kept.lock().expect("the requests"))
}

/// The only request the provider has been handed.
fn only(kept: &Kept) -> FlushRequest<V> {
    let mut requests = taken(kept);
    assert_eq!(requests.len(), 1, "{requests:?}");
    requests.remove(0)
}

fn succeed(request: FlushRequest<V>) {
    for part in request.domains {
        part.completion.succeed();
    }
}

fn expect_durable(harness: &mut Harness, id: u64) {
    match harness.next_entry() {
        Entry::Durable { through } => assert_eq!(through, harness.epoch(id)),
        other => panic!("expected Durable through {id}, got {other:?}"),
    }
}

fn next_failed(harness: &mut Harness) -> Failed<V> {
    match harness.next_entry() {
        Entry::Failed(failed) => failed,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// Pop for a while, requiring nothing.
fn nothing_for_a_while(harness: &mut Harness) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(50) {
        assert!(harness.ring.pop().expect("pop").is_none());
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A provider that answers every domain it is asked about with success, inside `make_durable`.
#[derive(Debug)]
struct Answers(Vec<FlushDomain>);

impl DurabilityProvider<V> for Answers {
    fn domains(&self) -> Vec<FlushDomain> {
        self.0.clone()
    }

    fn make_durable(&mut self, request: FlushRequest<V>) {
        succeed(request);
    }
}

/// A provider that panics when asked anything.
#[derive(Debug)]
struct Panics(Vec<FlushDomain>);

impl DurabilityProvider<V> for Panics {
    fn domains(&self) -> Vec<FlushDomain> {
        self.0.clone()
    }

    fn make_durable(&mut self, _: FlushRequest<V>) {
        panic!("the provider panics");
    }
}

#[test]
fn a_provider_answering_inside_make_durable_makes_the_seal_durable() {
    let temp = temp(&[0; 16]);
    let provider = Answers(vec![domain("log")]);
    let mut harness = instance(&temp, &["log"], Some(Box::new(provider)));
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    expect_durable(&mut harness, 1);
    assert_eq!(harness.state(1), EpochState::Durable);
    harness.finish();
}

#[test]
fn a_request_a_drain_makes_is_made_at_a_pop() {
    // Sealed with its write in flight -- the engine held, so no drain completes the write first --
    // the seal's request is made as the write's completion is drained, off the consumer's thread,
    // and waits for a consumer call (DI-D-44). Here the only calls are pops.
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = instance(&temp, &["log"], provider);
    let epoch = harness.epoch(1);
    let id = {
        let mut engine = harness.ring.engine();
        let id = engine
            .write(
                GIVEN,
                0,
                SubmissionData::Owned(b"log".to_vec()),
                epoch,
                1,
                WriteOptions::new(),
            )
            .expect("accepted");
        let (answer, _) = engine.make_durable_through(epoch).expect("seal");
        assert_eq!(answer, DurabilityRequest::Submitted);
        id
    };
    harness
        .oracle
        .pushed(id, OpKind::Write { epoch }, 3, 1)
        .expect("the oracle accepts the push");
    harness.oracle.sealed(epoch);
    assert!(
        taken(&kept).is_empty(),
        "the write was in flight at the seal"
    );

    harness.next();
    let started = Instant::now();
    let request = loop {
        if let Some(request) = taken(&kept).pop() {
            break request;
        }
        assert!(started.elapsed() < BOUND, "no pop made the drain's request");
        assert!(harness.ring.pop().expect("pop").is_none());
        std::thread::sleep(Duration::from_millis(1));
    };
    succeed(request);
    expect_durable(&mut harness, 1);
    harness.finish();
}

#[test]
fn a_request_names_each_domain_its_files_and_their_writes() {
    let temps = [temp(&[0; 16]), temp(&[0; 16]), temp(&[0; 16])];
    let files = vec![
        file(2, &temps[0], &["log"]),
        file(3, &temps[1], &["log"]),
        file(4, &temps[2], &["disk"]),
    ];
    let (provider, kept) = keeper(&["log"]);
    let ring = Ring::new(setup(files, Vec::new(), provider))
        .expect("build")
        .0;
    let mut harness = Harness::new(ring);
    let first = harness.write(IoTargetKey(2), 0, b"one", 1, 1);
    let other = harness.write(IoTargetKey(3), 0, b"two", 1, 2);
    let second = harness.write(IoTargetKey(2), 8, b"three", 1, 3);
    harness.write(IoTargetKey(4), 0, b"four", 1, 4);
    harness.next_n(4);
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);

    let request = only(&kept);
    assert_eq!(request.domains.len(), 1, "disk is the default's");
    let part = &request.domains[0];
    assert_eq!(part.domain.bytes(), b"log");
    let named: Vec<_> = part
        .files
        .iter()
        .map(|f| (f.key, f.writes.clone()))
        .collect();
    assert_eq!(
        named,
        [
            (IoTargetKey(2), vec![first, second]),
            (IoTargetKey(3), vec![other])
        ]
    );
    assert_eq!(harness.state(1), EpochState::Pending);

    succeed(request);
    expect_durable(&mut harness, 1);
    harness.finish();
}

#[test]
fn a_seal_touching_no_served_domain_asks_the_provider_nothing() {
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = instance(&temp, &["disk"], provider);
    harness.write(GIVEN, 0, b"disk", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    expect_durable(&mut harness, 1);
    assert!(taken(&kept).is_empty());
    harness.finish();
}

#[test]
fn an_answer_from_another_thread_sets_the_readiness_signal() {
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = instance(&temp, &["log"], provider);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let request = only(&kept);
    assert!(harness.ring.pop().expect("pop").is_none());

    let signal = harness.ring.readiness().expect("the readiness signal");
    signal.reset().expect("withdraw any stale signal");
    let (woken, wakes) = mpsc::channel();
    let woken = Mutex::new(woken);
    let wait = ThreadpoolWait::new(
        signal.into(),
        move |_| {
            if let Ok(woken) = woken.lock() {
                let _ = woken.send(());
            }
        },
        None,
    )
    .expect("a wait on the signal");
    wait.arm(None);
    std::thread::spawn(move || succeed(request))
        .join()
        .expect("the answering thread");
    let set = wakes.recv_timeout(BOUND);
    wait.stop_and_drain();
    set.expect("the answer made an entry poppable and set the signal");
    expect_durable(&mut harness, 1);
    harness.finish();
}

#[test]
fn a_file_both_answer_for_waits_for_the_provider_after_its_flush() {
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = instance(&temp, &["log", "disk"], provider);
    harness.write(GIVEN, 0, b"both", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let request = only(&kept);
    nothing_for_a_while(&mut harness);
    assert_eq!(harness.state(1), EpochState::Pending);
    succeed(request);
    expect_durable(&mut harness, 1);
    harness.finish();
}

#[test]
fn a_provider_failure_is_reported_with_its_domain_and_error_and_holds_the_seal() {
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = instance(&temp, &["log"], provider);
    let write = harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    let mut request = only(&kept);
    request
        .domains
        .remove(0)
        .completion
        .fail(std::io::Error::from_raw_os_error(IO_DEVICE));

    let failed = next_failed(&mut harness);
    match &failed.cause {
        FailureCause::Provider { domain, error } => {
            assert_eq!(domain.bytes(), b"log");
            assert_eq!(error.raw_os_error(), Some(IO_DEVICE));
        }
        other => panic!("expected a provider failure, got {other:?}"),
    }
    let suspects: Vec<_> = failed.suspect.writes().iter().map(|w| w.op).collect();
    assert_eq!(suspects, [write]);
    assert_eq!(harness.state(1), EpochState::Blocked(failed.id));
    assert!(harness.ring.pop().expect("pop").is_none(), "one answer");
    harness.finish();
}

#[test]
fn a_completion_dropped_unanswered_is_reported_as_abandoned() {
    let temp = temp(&[0; 16]);
    let (provider, kept) = keeper(&["log"]);
    let mut harness = instance(&temp, &["log"], provider);
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    drop(only(&kept));

    let failed = next_failed(&mut harness);
    assert!(
        matches!(&failed.cause, FailureCause::ProviderAbandoned { domain } if domain.bytes() == b"log"),
        "{:?}",
        failed.cause
    );
    assert_eq!(harness.state(1), EpochState::Blocked(failed.id));
    harness.finish();
}

#[test]
fn a_panic_in_the_provider_reaches_the_consumers_call_and_abandons_what_it_dropped() {
    let temp = temp(&[0; 16]);
    let provider = Panics(vec![domain("log")]);
    let mut harness = instance(&temp, &["log"], Some(Box::new(provider)));
    harness.write(GIVEN, 0, b"log", 1, 1);
    harness.next();
    let at = harness.default.at(1);
    let sealing = panic::catch_unwind(AssertUnwindSafe(|| harness.ring.make_durable_through(at)));
    assert!(sealing.is_err(), "the provider's panic propagates");
    harness.oracle.sealed(harness.epoch(1));

    let failed = next_failed(&mut harness);
    assert!(
        matches!(&failed.cause, FailureCause::ProviderAbandoned { domain } if domain.bytes() == b"log"),
        "{:?}",
        failed.cause
    );
    assert_eq!(
        harness.seal(1),
        DurabilityRequest::AlreadySealed(EpochState::Blocked(failed.id)),
        "the instance is still usable"
    );
    harness.finish();
}
