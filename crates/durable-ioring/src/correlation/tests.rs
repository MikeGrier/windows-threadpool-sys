// Copyright (c) 2026 Mike Grier
//! Correlation ids (DI-19.1): distinct across threads, rising on one, and minted only for a pair a
//! sink wants.

use std::collections::HashSet;
use std::sync::atomic::AtomicU64;
use std::thread;

use super::{CorrelationId, FIRST, mint_for};
use crate::diagnostics::{Diagnostic, DiagnosticKind, DiagnosticSink, NoDiagnostics};

/// A sink that wants only the kinds it was given.
struct Wants(Vec<DiagnosticKind>);

impl DiagnosticSink for Wants {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        self.0.contains(&kind)
    }

    fn report(&self, _: &Diagnostic<'_>) {}
}

fn everything() -> Wants {
    Wants(vec![
        DiagnosticKind::Inconsistency,
        DiagnosticKind::LockWait,
        DiagnosticKind::FlushIssued,
        DiagnosticKind::FlushAnswered,
        DiagnosticKind::SealWaited,
    ])
}

#[test]
fn a_counter_mints_from_one() {
    let counter = AtomicU64::new(FIRST);
    let id = mint_for(&counter, &everything(), DiagnosticKind::LockWait).expect("wanted");
    assert_eq!(id.get(), 1);
}

#[test]
fn ids_minted_on_one_thread_rise() {
    let counter = AtomicU64::new(FIRST);
    let sink = everything();
    let ids: Vec<u64> = (0..100)
        .map(|_| {
            mint_for(&counter, &sink, DiagnosticKind::FlushIssued)
                .expect("wanted")
                .get()
        })
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");
    assert_eq!(
        ids,
        (1..=100).collect::<Vec<_>>(),
        "one apart, from the first"
    );
}

#[test]
fn ids_from_the_process_counter_on_one_thread_rise() {
    let sink = everything();
    let first = CorrelationId::for_pair(&sink, DiagnosticKind::LockWait).expect("wanted");
    let second = CorrelationId::for_pair(&sink, DiagnosticKind::LockWait).expect("wanted");
    assert!(first < second, "{first} then {second}");
}

#[test]
fn ids_minted_on_many_threads_at_once_are_all_distinct() {
    const THREADS: usize = 8;
    const EACH: usize = 1_000;
    let counter = AtomicU64::new(FIRST);
    let ids: Vec<u64> = thread::scope(|scope| {
        let minting: Vec<_> = (0..THREADS)
            .map(|_| {
                scope.spawn(|| {
                    let sink = everything();
                    (0..EACH)
                        .map(|_| {
                            mint_for(&counter, &sink, DiagnosticKind::SealWaited)
                                .expect("wanted")
                                .get()
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        minting
            .into_iter()
            .flat_map(|handle| handle.join().expect("a minting thread"))
            .collect()
    });
    let distinct: HashSet<u64> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), THREADS * EACH, "no id minted twice");
    let expected: HashSet<u64> = (1..=(THREADS * EACH) as u64).collect();
    assert_eq!(distinct, expected, "none skipped");
}

#[test]
fn the_process_counter_gives_distinct_ids_to_many_threads() {
    let ids: Vec<CorrelationId> = thread::scope(|scope| {
        let minting: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let sink = everything();
                    (0..250)
                        .map(|_| {
                            CorrelationId::for_pair(&sink, DiagnosticKind::FlushAnswered)
                                .expect("wanted")
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        minting
            .into_iter()
            .flat_map(|handle| handle.join().expect("a minting thread"))
            .collect()
    });
    let distinct: HashSet<CorrelationId> = ids.iter().copied().collect();
    assert_eq!(distinct.len(), ids.len());
}

#[test]
fn a_sink_that_wants_nothing_takes_no_id() {
    let counter = AtomicU64::new(FIRST);
    for kind in everything().0 {
        assert_eq!(mint_for(&counter, &NoDiagnostics, kind), None);
    }
    let next = mint_for(&counter, &everything(), DiagnosticKind::LockWait).expect("wanted");
    assert_eq!(next.get(), FIRST, "nothing was taken before");
}

#[test]
fn a_sink_that_wants_some_kinds_takes_ids_only_for_those() {
    let counter = AtomicU64::new(FIRST);
    let sink = Wants(vec![DiagnosticKind::FlushIssued]);
    assert_eq!(mint_for(&counter, &sink, DiagnosticKind::LockWait), None);
    let wanted = mint_for(&counter, &sink, DiagnosticKind::FlushIssued).expect("wanted");
    assert_eq!(mint_for(&counter, &sink, DiagnosticKind::SealWaited), None);
    let again = mint_for(&counter, &sink, DiagnosticKind::FlushIssued).expect("wanted");
    assert_eq!(
        (wanted.get(), again.get()),
        (1, 2),
        "the declined kinds took none between"
    );
}

#[test]
fn the_process_counter_takes_nothing_for_a_sink_that_wants_nothing() {
    for kind in everything().0 {
        assert_eq!(CorrelationId::for_pair(&NoDiagnostics, kind), None);
    }
}

#[test]
fn the_counter_wraps_as_fetch_add_does() {
    let counter = AtomicU64::new(u64::MAX);
    let sink = everything();
    let last = mint_for(&counter, &sink, DiagnosticKind::LockWait).expect("wanted");
    let wrapped = mint_for(&counter, &sink, DiagnosticKind::LockWait).expect("wanted");
    assert_eq!((last.get(), wrapped.get()), (u64::MAX, 0));
}

#[test]
fn an_id_shows_as_its_number() {
    let counter = AtomicU64::new(41);
    let id = mint_for(&counter, &everything(), DiagnosticKind::LockWait).expect("wanted");
    assert_eq!(id.to_string(), "41");
    assert_eq!(id.get(), 41);
    assert_eq!(format!("{id:?}"), "CorrelationId(41)");
}
