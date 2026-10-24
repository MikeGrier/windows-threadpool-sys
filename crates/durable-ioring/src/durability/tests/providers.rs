// Copyright (c) 2026 Mike Grier
//! The consumer's durability provider in the core (DI-D-27, DI-D-44): what a seal asks it, how its
//! answers decide the seal, what a failure of its domain reaches, and the answers that are not
//! consistent with any request.

use std::io;

use super::Stamp;
use super::{
    A, B, C, DEFAULT, FULL, Lineage, Ops, Seen, State, durable, flush, inconsistent, lineage,
    quiet, seen, submitted,
};
use crate::domains::DomainId;
use crate::durability::{DEFAULT_LINEAGE, DomainPart, Due, Key, ProviderCall, Reach, Routing, V};
use crate::types::{FailureCause, FlushDomain, ImportScope, MarkingKind};

/// A file only the provider answers for: every domain it declares is served.
const PROVIDER: Routing = Routing {
    default: false,
    provider: true,
};
/// A file both answer for: it declares a served domain and one the provider does not serve.
const BOTH: Routing = Routing {
    default: true,
    provider: true,
};

/// A covered marking citing the provider's warranty alone: no flush was made for the file.
const BY_PROVIDER: MarkingKind = MarkingKind::Covered {
    flushed: false,
    provider: true,
};
/// A covered marking citing both the default's observed flush and the provider's warranty.
const BY_BOTH: MarkingKind = MarkingKind::Covered {
    flushed: true,
    provider: true,
};

fn serve(lineage: &mut Lineage, ids: &[u32]) {
    lineage.serve(
        ids.iter().map(|&d| DomainId(d)).collect(),
        ids.iter()
            .map(|d| crate::types::FlushDomain::new(format!("d{d}").into_bytes()))
            .collect(),
    );
}

/// The single request a change asked of the provider.
fn call<'a>(due: &'a Due<u64, &'static str, Stamp>) -> &'a ProviderCall {
    assert_eq!(due.calls.len(), 1, "one request: {:?}", due.calls);
    &due.calls[0]
}

fn succeeded(lineage: &mut Lineage, seal: u64, domain: u32) -> Due<u64, &'static str, Stamp> {
    lineage.provided(DEFAULT_LINEAGE, seal, DomainId(domain), Ok(()))
}

fn provider_failure(domain: u32) -> FailureCause<V<u64, Stamp>> {
    FailureCause::Provider {
        domain: FlushDomain::new(domain.to_le_bytes()),
        error: crate::report_io_error(io::Error::from_raw_os_error(1117)),
    }
}

fn failed(lineage: &mut Lineage, seal: u64, domain: u32) -> Due<u64, &'static str, Stamp> {
    lineage.provided(
        DEFAULT_LINEAGE,
        seal,
        DomainId(domain),
        Err(provider_failure(domain)),
    )
}

/// A failure observed before the seal, so a write's coverage shows as a marking.
fn import(lineage: &mut Lineage) {
    lineage.import(
        FailureCause::Imported {
            scope: ImportScope::All,
        },
        &Reach::All,
    );
}

#[test]
fn a_seal_asks_the_provider_once_with_its_domains_sorted_and_each_files_writes_grouped() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1, 2, 3]);
    let first = ops.push_in(&mut lineage, 1, A, PROVIDER, &[2, 1]);
    let other = ops.push_in(&mut lineage, 1, B, PROVIDER, &[1]);
    let second = ops.push_in(&mut lineage, 1, A, PROVIDER, &[2, 1]);
    let mixed = ops.push_in(&mut lineage, 1, C, BOTH, &[3, 9]);
    for write in [first, other, second, mixed] {
        lineage.completed(write, FULL);
    }
    let due = submitted(lineage.seal(1));
    assert_eq!(
        due.flushes,
        [flush(1, C)],
        "9 is not served: C is the default's too"
    );
    let asked = call(&due);
    assert_eq!(asked.lineage, DEFAULT_LINEAGE);
    assert_eq!(
        asked.domains,
        [
            DomainPart {
                domain: DomainId(1),
                files: vec![(A, vec![first, second]), (B, vec![other])],
            },
            DomainPart {
                domain: DomainId(2),
                files: vec![(A, vec![first, second])],
            },
            DomainPart {
                domain: DomainId(3),
                files: vec![(C, vec![mixed])],
            },
        ]
    );
}

#[test]
fn a_seal_whose_writes_touch_no_served_domain_asks_the_provider_nothing() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1]);
    let write = ops.push_in(&mut lineage, 1, A, DEFAULT, &[9]);
    lineage.completed(write, FULL);
    let due = submitted(lineage.seal(1));
    assert!(due.calls.is_empty());
    assert_eq!(due.flushes, [flush(1, A)]);
    assert_eq!(durable(&lineage.flushed(1, A, Ok(()))), [1]);
}

#[test]
fn a_seal_naming_no_write_asks_the_provider_nothing() {
    let mut lineage = lineage();
    serve(&mut lineage, &[1]);
    let due = submitted(lineage.seal(1));
    assert!(due.calls.is_empty());
    assert_eq!(durable(&due), [1]);
}

#[test]
fn each_seal_is_its_own_request() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1]);
    let early = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1]);
    let late = ops.push_in(&mut lineage, 2, A, PROVIDER, &[1]);
    lineage.completed(early, FULL);
    lineage.completed(late, FULL);
    let one = call(&submitted(lineage.seal(1))).clone();
    let two = call(&submitted(lineage.seal(2))).clone();
    assert_ne!(one.seal, two.seal);
    assert_eq!(one.domains[0].files, [(A, vec![early])]);
    assert_eq!(two.domains[0].files, [(A, vec![late])]);
    assert_eq!(
        durable(&succeeded(&mut lineage, two.seal, 1)),
        Vec::<u64>::new()
    );
    assert_eq!(lineage.state(2), State::Pending, "1 is still pending");
    assert_eq!(durable(&succeeded(&mut lineage, one.seal, 1)), [1, 2]);
}

#[test]
fn a_provider_success_makes_a_seal_it_alone_answers_for_durable() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1]);
    let write = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1]);
    lineage.completed(write, FULL);
    let due = submitted(lineage.seal(1));
    assert!(due.flushes.is_empty());
    let seal = call(&due).seal;
    assert_eq!(lineage.state(1), State::Pending);
    assert_eq!(seen(&succeeded(&mut lineage, seal, 1)), [Seen::Durable(1)]);
    assert_eq!(lineage.durable_through(), Some(1));
}

#[test]
fn a_seal_waits_for_every_domain_it_asked_about() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1, 2]);
    let write = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1, 2]);
    lineage.completed(write, FULL);
    let seal = call(&submitted(lineage.seal(1))).seal;
    assert!(quiet(&succeeded(&mut lineage, seal, 2)));
    assert_eq!(lineage.state(1), State::Pending);
    assert_eq!(durable(&succeeded(&mut lineage, seal, 1)), [1]);
}

#[test]
fn a_file_both_answer_for_needs_its_flush_and_its_domain_in_either_order() {
    for flush_first in [true, false] {
        let mut lineage = lineage();
        let mut ops = Ops::new();
        serve(&mut lineage, &[1]);
        let write = ops.push_in(&mut lineage, 1, A, BOTH, &[1, 2]);
        lineage.completed(write, FULL);
        import(&mut lineage);
        let due = submitted(lineage.seal(1));
        assert_eq!(due.flushes, [flush(1, A)]);
        let seal = call(&due).seal;
        let (first, last) = if flush_first {
            let first = lineage.flushed(1, A, Ok(()));
            (first, succeeded(&mut lineage, seal, 1))
        } else {
            let first = succeeded(&mut lineage, seal, 1);
            (first, lineage.flushed(1, A, Ok(())))
        };
        assert!(quiet(&first), "half covered is not covered ({flush_first})");
        assert_eq!(
            seen(&last),
            [Seen::Marked(0, write.seq, BY_BOTH), Seen::Blocked(1, 0)],
            "covered, citing both, and held by the import ({flush_first})"
        );
    }
}

#[test]
fn a_provider_success_covers_the_writes_it_completes() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1, 2]);
    let on_one = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1]);
    let on_two = ops.push_in(&mut lineage, 1, B, PROVIDER, &[2]);
    lineage.completed(on_one, FULL);
    lineage.completed(on_two, FULL);
    import(&mut lineage);
    let seal = call(&submitted(lineage.seal(1))).seal;
    assert_eq!(
        seen(&succeeded(&mut lineage, seal, 2)),
        [Seen::Marked(0, on_two.seq, BY_PROVIDER)],
        "domain 2 completes B's write and not A's"
    );
    assert_eq!(
        seen(&succeeded(&mut lineage, seal, 1)),
        [
            Seen::Marked(0, on_one.seq, BY_PROVIDER),
            Seen::Blocked(1, 0)
        ]
    );
}

#[test]
fn a_provider_failure_reaches_the_files_in_its_domain_and_every_unknown_file() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1, 2]);
    let in_one = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1]);
    let in_two = ops.push_in(&mut lineage, 1, B, PROVIDER, &[2]);
    let unknown = ops.push(&mut lineage, 1, C, DEFAULT);
    for write in [in_one, in_two, unknown] {
        lineage.completed(write, FULL);
    }
    let due = submitted(lineage.seal(1));
    assert_eq!(due.flushes, [flush(1, C)]);
    let seal = call(&due).seal;
    assert_eq!(
        seen(&failed(&mut lineage, seal, 1)),
        [Seen::Failed(0, vec![in_one.seq, unknown.seq])],
        "B is in domain 2 alone; C declared no domain"
    );
    let inventory = lineage.failures();
    assert!(
        matches!(
            &inventory[0].cause,
            FailureCause::Provider { domain, .. } if domain.bytes() == 1u32.to_le_bytes()
        ),
        "{:?}",
        inventory[0].cause
    );
    assert!(seen(&succeeded(&mut lineage, seal, 2)).is_empty());
    assert_eq!(
        seen(&lineage.flushed(1, C, Ok(()))),
        [Seen::Marked(0, unknown.seq, super::COVERED)],
        "the failure is the seal's answer; nothing more is reported for it"
    );
    assert_eq!(lineage.durable_through(), None);
    assert!(matches!(lineage.state(1), State::Blocked(by) if by.seq == 0));
}

#[test]
fn a_completion_dropped_unanswered_is_a_failure_of_its_domain() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1]);
    let write = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1]);
    lineage.completed(write, FULL);
    let seal = call(&submitted(lineage.seal(1))).seal;
    let abandoned = FailureCause::ProviderAbandoned {
        domain: FlushDomain::new(*b"one"),
    };
    let due = lineage.provided(DEFAULT_LINEAGE, seal, DomainId(1), Err(abandoned));
    assert_eq!(seen(&due), [Seen::Failed(0, vec![write.seq])]);
    assert!(matches!(
        &lineage.failures()[0].cause,
        FailureCause::ProviderAbandoned { domain } if domain.bytes() == b"one"
    ));
    assert!(matches!(lineage.state(1), State::Blocked(by) if by.seq == 0));
}

#[test]
fn a_minted_lineages_seal_asks_with_its_own_key() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1]);
    let minted: Key = lineage.mint(Some("log".to_owned()));
    let write = ops.push_to(&mut lineage, minted, 1, A, PROVIDER, &[1]);
    lineage.completed(write, FULL);
    let asked = call(&submitted(lineage.0.seal(minted, 1))).clone();
    assert_eq!(asked.lineage, minted);
    inconsistent("a provider answered for a seal that does not exist", || {
        lineage.provided(DEFAULT_LINEAGE, asked.seal, DomainId(1), Ok(()))
    });
    assert_eq!(
        seen(&lineage.provided(minted, asked.seal, DomainId(1), Ok(()))),
        [Seen::DurableIn(minted, 1)]
    );
}

#[test]
fn an_answer_for_a_lineage_that_has_ended_reports_nothing() {
    let mut lineage = lineage();
    let mut ops = Ops::new();
    serve(&mut lineage, &[1]);
    let minted = lineage.mint(None);
    let write = ops.push_to(&mut lineage, minted, 1, A, PROVIDER, &[1]);
    lineage.completed(write, FULL);
    let seal = call(&submitted(lineage.0.seal(minted, 1))).seal;
    // A write still in flight keeps the ended lineage recorded (DI-D-42), so the answer finds it
    // and must still report nothing.
    ops.push_to(&mut lineage, minted, 2, A, PROVIDER, &[1]);
    lineage.end(minted);
    let failure = Err(provider_failure(1));
    assert!(quiet(&lineage.provided(minted, seal, DomainId(1), failure)));
    assert!(
        lineage.failures().is_empty(),
        "an ended lineage's answer is not observed: it is late, not inconsistent"
    );
}

#[test]
fn an_answer_no_request_asked_for_is_inconsistent() {
    let setup = || {
        let mut lineage = lineage();
        let mut ops = Ops::new();
        serve(&mut lineage, &[1, 2]);
        let write = ops.push_in(&mut lineage, 1, A, PROVIDER, &[1, 2]);
        lineage.completed(write, FULL);
        let seal = call(&submitted(lineage.seal(1))).seal;
        (lineage, seal, ops)
    };

    let (mut lineage, seal, _) = setup();
    inconsistent("a provider answered for a seal that does not exist", || {
        succeeded(&mut lineage, seal + 7, 1)
    });

    let (mut lineage, seal, _) = setup();
    inconsistent(
        "a provider answered a domain the seal did not ask about",
        || succeeded(&mut lineage, seal, 3),
    );
    assert_eq!(
        lineage.state(1),
        State::Pending,
        "the stray answer decided nothing"
    );

    let (mut lineage, seal, _) = setup();
    assert!(quiet(&succeeded(&mut lineage, seal, 1)));
    inconsistent("a provider answered a domain twice", || {
        failed(&mut lineage, seal, 1)
    });
    assert!(
        lineage.failures().is_empty(),
        "the second answer is not observed"
    );

    // A seal that has finished but is held behind a pending one is still kept; one that has been
    // reported durable is not, and an answer for it is for a seal that does not exist.
    let (mut lineage, _, mut ops) = setup();
    let later = ops.push_in(&mut lineage, 2, B, PROVIDER, &[1]);
    lineage.completed(later, FULL);
    let second = call(&submitted(lineage.seal(2))).seal;
    assert!(quiet(&succeeded(&mut lineage, second, 1)));
    inconsistent("a provider answered for a seal not flushing", || {
        succeeded(&mut lineage, second, 1)
    });

    let (mut lineage, seal, _) = setup();
    succeeded(&mut lineage, seal, 1);
    assert_eq!(durable(&succeeded(&mut lineage, seal, 2)), [1]);
    inconsistent("a provider answered for a seal that does not exist", || {
        succeeded(&mut lineage, seal, 2)
    });
}
