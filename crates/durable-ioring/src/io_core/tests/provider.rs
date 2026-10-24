// Copyright (c) 2026 Mike Grier
//! The consumer's provider (DI-D-27, DI-D-50 P6): routing, requests and answers.

use std::io;
use std::sync::Arc;

use super::{Core, KEY, OTHER, Rig, Ticks, shown};
use crate::backend::SubmissionOp;
use crate::diagnostics::NoDiagnostics;
use crate::io_core::DuplicateDomain;
use crate::memory::MemoryTarget;
use crate::provider::DomainOutcome;
use crate::types::{Entry, FailureCause, FlushDomain, IoTargetKey};

fn domain(name: &str) -> FlushDomain {
    FlushDomain::new(name.as_bytes())
}

/// A rig whose provider serves `served`, with `KEY` declared in `declared`.
fn served(served: &[&str], declared: &[&str]) -> Rig {
    let core = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        served.iter().map(|name| domain(name)).collect(),
    )
    .expect("no repeats");
    let mut rig = Rig::over(core);
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains(declared.iter().map(|name| domain(name))),
    );
    rig
}

/// Write at epoch 1 and seal it, running until only the provider is left to answer.
fn sealed(rig: &mut Rig) {
    rig.write(1, 0, b"x").expect("accepted");
    rig.seal(1);
    rig.run();
}

#[test]
fn a_domain_named_twice_is_refused() {
    let refused = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        vec![domain("log"), domain("disk"), domain("log")],
    )
    .err()
    .expect("a repeat");
    assert_eq!(refused, DuplicateDomain(domain("log")));
    assert!(refused.to_string().contains("twice"));
}

#[test]
fn a_seal_asks_the_provider_for_a_served_domain_and_commits_nothing_for_it() {
    let mut rig = served(&["log"], &["log"]);
    rig.write(1, 0, b"a").expect("accepted");
    rig.write(1, 1, b"b").expect("accepted");
    rig.run();
    let (_, actions) = rig.sealed(rig.epoch(1));
    assert!(actions.submit.is_empty(), "no commit: the provider answers");
    let [request] = actions.requests.as_slice() else {
        panic!("one request: {:?}", actions.requests);
    };
    assert_eq!(request.lineage, rig.core.default_lineage());
    let [part] = request.domains.as_slice() else {
        panic!("one part");
    };
    assert_eq!(part.domain, domain("log"));
    assert_eq!(part.targets.len(), 1);
    assert_eq!(part.targets[0].key, KEY);
    assert_eq!(part.targets[0].writes.len(), 2);
}

#[test]
fn a_provider_success_makes_the_seal_durable() {
    let mut rig = served(&["log"], &["log"]);
    sealed(&mut rig);
    rig.entries();
    let request = rig.requests.pop().expect("a request");
    let actions = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("log"),
        DomainOutcome::Succeeded,
    );
    assert!(actions.deliver);
    rig.act(actions);
    assert_eq!(shown(&mut rig), ["durable 1"]);
    assert!(
        rig.target().durable_contents().is_empty(),
        "the provider answered; the target was never committed"
    );
}

#[test]
fn a_target_with_a_served_and_an_unserved_domain_needs_both() {
    let mut rig = served(&["log"], &["log", "disk"]);
    rig.write(1, 0, b"x").expect("accepted");
    rig.run();
    rig.entries();
    let (_, actions) = rig.sealed(rig.epoch(1));
    assert_eq!(actions.requests.len(), 1);
    assert_eq!(actions.submit.len(), 1);
    assert!(matches!(actions.submit[0].op, SubmissionOp::Commit));
    rig.act(actions);
    rig.run();
    assert!(
        rig.entries().is_empty(),
        "the commit alone does not make it durable"
    );
    let request = rig.requests.pop().expect("a request");
    let actions = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("log"),
        DomainOutcome::Succeeded,
    );
    rig.act(actions);
    assert_eq!(shown(&mut rig), ["durable 1"]);
}

#[test]
fn a_target_with_no_domains_is_the_defaults_alone() {
    let mut rig = served(&["log"], &[]);
    sealed(&mut rig);
    assert!(rig.requests.is_empty());
    assert_eq!(
        shown(&mut rig).last().map(String::as_str),
        Some("durable 1")
    );
}

#[test]
fn a_provider_failure_is_a_failure_of_its_domain() {
    let mut rig = served(&["log"], &["log"]);
    sealed(&mut rig);
    rig.entries();
    let request = rig.requests.pop().expect("a request");
    let actions = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("log"),
        DomainOutcome::Failed(crate::report_io_error(io::Error::other("lost"))),
    );
    rig.act(actions);
    let entries = rig.entries();
    let Some(Entry::Failed(failed)) = entries.first() else {
        panic!("a failure: {entries:?}");
    };
    let FailureCause::Provider {
        domain: failed_domain,
        error,
    } = &failed.cause
    else {
        panic!("the provider's: {:?}", failed.cause);
    };
    assert_eq!(*failed_domain, domain("log"));
    assert_eq!(error.to_string(), "lost");
}

#[test]
fn an_abandoned_answer_is_the_providers_abandonment() {
    let mut rig = served(&["log"], &["log"]);
    sealed(&mut rig);
    rig.entries();
    let request = rig.requests.pop().expect("a request");
    let actions = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("log"),
        DomainOutcome::Abandoned,
    );
    rig.act(actions);
    let entries = rig.entries();
    assert!(
        matches!(
            entries.first(),
            Some(Entry::Failed(failed)) if matches!(failed.cause, FailureCause::ProviderAbandoned { .. })
        ),
        "{entries:?}"
    );
}

#[test]
fn an_answer_for_an_ended_lineage_reports_nothing() {
    let mut rig = served(&["log"], &["log"]);
    let lineage = rig.core.mint_lineage(None);
    rig.write_with(
        KEY,
        crate::types::Epoch::new(lineage, 1),
        0,
        b"x",
        Default::default(),
    )
    .expect("accepted");
    rig.seal_in(lineage, 1);
    rig.run();
    let request = rig.requests.pop().expect("a request");
    let actions = rig.core.end_lineage(lineage).expect("live");
    rig.act(actions);
    rig.entries();
    let actions = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("log"),
        DomainOutcome::Succeeded,
    );
    assert!(actions.is_empty());
    assert!(rig.entries().is_empty());
}

#[test]
fn each_target_in_a_domain_is_named_with_its_writes() {
    let mut rig = served(&["log"], &["log"]);
    rig.attach(OTHER, MemoryTarget::new().with_domains([domain("log")]));
    rig.write(1, 0, b"a").expect("accepted");
    rig.write_to(OTHER, 1, 0, b"b").expect("accepted");
    rig.write(1, 1, b"c").expect("accepted");
    rig.seal(1);
    rig.run();
    let request = rig.requests.pop().expect("a request");
    let named: Vec<(IoTargetKey, usize)> = request.domains[0]
        .targets
        .iter()
        .map(|target| (target.key, target.writes.len()))
        .collect();
    assert_eq!(named, [(KEY, 2), (OTHER, 1)]);
}

#[test]
#[should_panic(expected = "a provider answered for a request this core never made")]
fn an_answer_for_an_unserved_domain_is_panicked_on() {
    // Declared, so the core knows the domain, but not served, so no request asks about it.
    let mut rig = served(&["log"], &["log", "disk"]);
    sealed(&mut rig);
    let request = rig.requests.pop().expect("a request");
    let _ = rig.core.provided(
        request.lineage,
        request.seal,
        &domain("disk"),
        DomainOutcome::Succeeded,
    );
}

#[test]
#[should_panic(expected = "a provider answered for a request this core never made")]
fn an_answer_for_another_cores_lineage_is_panicked_on() {
    let mut rig = served(&["log"], &["log"]);
    let other = Core::new(Ticks::default(), Arc::new(NoDiagnostics));
    let _ = rig.core.provided(
        other.default_lineage(),
        0,
        &domain("log"),
        DomainOutcome::Succeeded,
    );
}

#[test]
#[should_panic(expected = "a provider answered a domain twice")]
fn a_domain_answered_twice_is_panicked_on() {
    // A second served domain left unanswered keeps the seal asking.
    let mut rig = served(&["log", "journal"], &["log", "journal"]);
    sealed(&mut rig);
    let request = rig.requests.pop().expect("a request");
    for _ in 0..2 {
        let _ = rig.core.provided(
            request.lineage,
            request.seal,
            &domain("log"),
            DomainOutcome::Succeeded,
        );
    }
}
