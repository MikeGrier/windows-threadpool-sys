// Copyright (c) 2026 Mike Grier
//! What the core refuses past the capacity it was built for (DI-D-107): construction, a push, a
//! mint, an import and an `add_target`, each refused whole, changing nothing, and each lifted by
//! letting go of something the count covers.

use std::sync::Arc;

use super::{Core, KEY, OTHER, Pushed, Rig, Ticks, V, domain, listed, nz};
use crate::capacity::{AtCapacity, Capacity, ConstructionError, Limit};
use crate::diagnostics::NoDiagnostics;
use crate::ids::FailureToken;
use crate::io_target::IoTarget;
use crate::memory::{Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When};
use crate::test_support::CAPACITY;
use crate::types::{
    AddTargetCause, Entry, Epoch, FailureCause, ImportError, ImportScope, IoTargetKey, PushRefusal,
    Resolution, WriteOptions,
};

fn declared(f: impl FnOnce(&mut Capacity)) -> Capacity {
    let mut capacity = CAPACITY;
    f(&mut capacity);
    capacity
}

fn core_with(capacity: Capacity) -> Core {
    Core::new(Ticks::default(), Arc::new(NoDiagnostics), capacity).expect("a valid capacity")
}

/// A rig over a core built with `capacity`, with `KEY` attached.
fn rig_with(capacity: Capacity) -> Rig {
    let mut rig = Rig::over(core_with(capacity));
    rig.attach(KEY, MemoryTarget::new());
    rig
}

/// A rig over a core built with `capacity` and a backend that does what `script` says.
fn scripted_rig(capacity: Capacity, script: FaultScript) -> Rig {
    let mut rig = Rig::over(core_with(capacity));
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(script);
    rig.attach(KEY, MemoryTarget::new());
    rig
}

/// Drain the queue, returning the failures' tokens in order.
fn tokens(rig: &mut Rig) -> Vec<FailureToken> {
    rig.entries()
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .collect()
}

fn at(limit: Limit, declared: usize) -> AtCapacity {
    AtCapacity { limit, declared }
}

fn refusal(pushed: Pushed) -> PushRefusal<V> {
    pushed.expect_err("refused").reason
}

/// A target declaring the flush domains `names`.
fn declaring(names: &[&str]) -> crate::io_target::IoTargetDescription {
    MemoryTarget::new()
        .with_domains(names.iter().map(|name| domain(name)))
        .describe()
}

#[test]
fn a_core_declaring_no_lineage_is_refused() {
    let none = declared(|c| c.lineages = 0);
    let refused = Core::new(Ticks::default(), Arc::new(NoDiagnostics), none);
    assert_eq!(refused.err(), Some(ConstructionError::NoLineages));
    let refused = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        none,
        vec![domain("log")],
    );
    assert_eq!(refused.err(), Some(ConstructionError::NoLineages));
}

#[test]
fn a_capacity_with_no_lineage_is_refused_before_the_provider_is_looked_at() {
    let none = declared(|c| {
        c.lineages = 0;
        c.domains = 0;
    });
    let served = vec![domain("a"), domain("a")];
    let refused = Core::with_provider(Ticks::default(), Arc::new(NoDiagnostics), none, served);
    assert_eq!(refused.err(), Some(ConstructionError::NoLineages));
}

#[test]
fn a_core_declaring_only_the_default_lineage_is_accepted() {
    let mut core = core_with(declared(|c| c.lineages = 1));
    assert_eq!(listed(&core).len(), 1);
    assert_eq!(
        core.mint_lineage(None).err(),
        Some(at(Limit::Lineages, 1)),
        "the default is the one"
    );
}

#[test]
fn a_provider_serving_more_domains_than_declared_is_refused() {
    let two = declared(|c| c.domains = 2);
    let served = vec![domain("a"), domain("b"), domain("c")];
    let refused = Core::with_provider(Ticks::default(), Arc::new(NoDiagnostics), two, served);
    assert_eq!(
        refused.err(),
        Some(ConstructionError::TooManyDomains {
            served: 3,
            declared: 2
        })
    );
}

#[test]
fn a_provider_serving_exactly_the_domains_declared_is_accepted() {
    let two = declared(|c| c.domains = 2);
    let served = vec![domain("a"), domain("b")];
    assert!(Core::with_provider(Ticks::default(), Arc::new(NoDiagnostics), two, served).is_ok());
}

#[test]
fn a_domain_named_twice_is_refused_before_the_count_is_taken() {
    let one = declared(|c| c.domains = 1);
    let served = vec![domain("a"), domain("a")];
    let refused = Core::with_provider(Ticks::default(), Arc::new(NoDiagnostics), one, served);
    assert_eq!(
        refused.err(),
        Some(ConstructionError::DuplicateDomain(domain("a")))
    );
}

#[test]
fn a_push_past_the_operations_declared_is_refused_and_hands_back_what_it_was_given() {
    let mut rig = rig_with(declared(|c| c.operations = 3));
    for epoch in 1..=3 {
        rig.write(epoch, 0, b"x").expect("room");
    }
    let refused = rig.write(4, 0, b"mine").expect_err("full");
    assert!(matches!(
        refused.reason,
        PushRefusal::AtCapacity(AtCapacity {
            limit: Limit::Operations,
            declared: 3
        })
    ));
    assert_eq!(refused.buffer.as_deref(), Some(b"mine".as_slice()));
    assert_eq!(refused.context, "write");
    assert_eq!(rig.core.window_places(), 3, "nothing was reserved");
}

#[test]
fn a_refused_push_spends_no_operation_id() {
    let mut rig = rig_with(declared(|c| c.operations = 1));
    let first = rig.write(1, 0, b"x").expect("room");
    assert!(matches!(
        refusal(rig.write(2, 0, b"y")),
        PushRefusal::AtCapacity(_)
    ));
    rig.run();
    rig.entries();
    rig.seal(1);
    rig.run();
    rig.entries();
    let next = rig.write(2, 0, b"y").expect("room again");
    assert_eq!(next.seq, first.seq + 1);
}

#[test]
fn a_read_is_refused_at_capacity_as_a_write_is() {
    let mut rig = rig_with(declared(|c| c.operations = 1));
    rig.read(0, 4).expect("room");
    assert!(matches!(
        refusal(rig.read(0, 4)),
        PushRefusal::AtCapacity(AtCapacity {
            limit: Limit::Operations,
            declared: 1
        })
    ));
}

#[test]
fn popping_a_reads_completion_lets_the_next_push_in() {
    let mut rig = rig_with(declared(|c| c.operations = 1));
    rig.read(0, 4).expect("room");
    rig.run();
    assert!(
        matches!(refusal(rig.read(0, 4)), PushRefusal::AtCapacity(_)),
        "completed and queued still holds the place"
    );
    rig.entries();
    rig.read(0, 4).expect("the place was let go");
}

#[test]
fn a_write_holds_its_place_until_it_is_covered_whatever_was_popped() {
    let mut rig = rig_with(declared(|c| c.operations = 1));
    rig.write(1, 0, b"x").expect("room");
    rig.run();
    rig.entries();
    assert!(
        matches!(refusal(rig.write(2, 0, b"y")), PushRefusal::AtCapacity(_)),
        "popped, and still uncovered"
    );
    rig.seal(1);
    rig.run();
    rig.entries();
    rig.write(2, 0, b"y")
        .expect("covered: the place was let go");
}

#[test]
fn an_operation_held_for_its_gate_takes_a_place() {
    let mut rig = rig_with(declared(|c| c.operations = 2));
    rig.write(1, 0, b"x").expect("room");
    let options = WriteOptions::new().gate(rig.epoch(1));
    let epoch = rig.epoch(2);
    rig.write_with(KEY, epoch, 1, b"y", options)
        .expect("held, and a place taken");
    assert_eq!(rig.core.window_places(), 2);
    let options = WriteOptions::new().gate(rig.epoch(1));
    let epoch = rig.epoch(3);
    let refused = rig
        .write_with(KEY, epoch, 2, b"z", options)
        .expect_err("full");
    assert!(matches!(refused.reason, PushRefusal::AtCapacity(_)));
}

#[test]
fn the_cores_own_refusals_come_before_the_capacity() {
    let mut rig = rig_with(declared(|c| c.operations = 1));
    rig.write(1, 0, b"x").expect("room");
    assert!(matches!(
        refusal(rig.write_to(OTHER, 2, 0, b"y")),
        PushRefusal::UnknownFile(OTHER)
    ));
    rig.seal(1);
    assert!(
        matches!(refusal(rig.write(1, 0, b"y")), PushRefusal::Sealed { .. }),
        "a write into a sealed epoch is refused as that, not as full"
    );
}

#[test]
fn no_operations_declared_refuses_every_push() {
    let mut rig = rig_with(declared(|c| c.operations = 0));
    assert!(matches!(
        refusal(rig.write(1, 0, b"x")),
        PushRefusal::AtCapacity(AtCapacity { declared: 0, .. })
    ));
    assert!(matches!(
        refusal(rig.read(0, 1)),
        PushRefusal::AtCapacity(_)
    ));
}

#[test]
fn a_refused_push_never_reaches_the_backend() {
    let mut rig = rig_with(declared(|c| c.operations = 0));
    let _ = rig.write(1, 0, b"x");
    assert_eq!(rig.backend.queued(), 0, "refused before the backend's push");
}

#[test]
fn a_seal_is_never_refused_for_capacity() {
    let mut rig = rig_with(declared(|c| c.operations = 1));
    rig.write(1, 0, b"x").expect("room");
    rig.run();
    assert!(matches!(
        refusal(rig.write(2, 0, b"y")),
        PushRefusal::AtCapacity(_)
    ));
    rig.seal(1);
    rig.run();
    assert_eq!(rig.core.window_places(), 1, "only the completion is queued");
}

/// The first commit fails and its failure stays unresolved, holding the mark: each seal after it
/// finishes and is not answered, and keeps the place of the write it covers (DI-D-110), so the
/// write / seal loop runs into the operations declared rather than growing the seal queue. Its four
/// recorded seals fill the seal table, made for the operations declared, exactly.
#[test]
fn seals_behind_a_mark_held_by_a_failure_keep_their_places_until_it_is_resolved() {
    let script = FaultScript::new().on(
        When::any().op(ScriptOp::Commit).nth(1),
        Fault::Fail(crate::memory::scripted_fault("the flush failed")),
    );
    let mut rig = scripted_rig(declared(|c| c.operations = 4), script);
    rig.write(1, 0, b"x").expect("room");
    rig.seal(1);
    rig.run();
    let mut made = tokens(&mut rig);
    assert_eq!(made.len(), 1, "the first commit failed");
    assert_eq!(
        rig.core.window_places(),
        1,
        "the failure suspects the write"
    );
    for epoch in 2..=4 {
        rig.write(epoch, 0, b"y").expect("room");
        rig.seal(epoch);
        rig.run();
        rig.entries();
        assert_eq!(
            rig.core.window_places(),
            epoch as usize,
            "the seal finished, and holds its write's place behind the mark"
        );
    }
    assert!(matches!(
        refusal(rig.write(5, 0, b"z")),
        PushRefusal::AtCapacity(AtCapacity {
            limit: Limit::Operations,
            declared: 4
        })
    ));
    let actions = rig
        .core
        .resolve(vec![(made.remove(0), Resolution::Abandon)])
        .expect("resolved");
    rig.act(actions);
    rig.run();
    rig.entries();
    assert_eq!(
        rig.core.window_places(),
        0,
        "the mark passed every seal, and each let go of its place"
    );
    rig.write(5, 0, b"z").expect("room again");
}

#[test]
fn a_mint_past_the_lineages_declared_is_refused_and_a_retire_lifts_it() {
    let mut core = core_with(declared(|c| c.lineages = 3));
    let first = core.mint_lineage(None).expect("room");
    let second = core.mint_lineage(None).expect("room");
    assert_eq!(core.mint_lineage(None).err(), Some(at(Limit::Lineages, 3)));
    assert_eq!(listed(&core).len(), 3, "nothing was minted");
    core.retire_lineage(second).expect("nothing holds it");
    let third = core.mint_lineage(None).expect("a place was let go");
    assert_ne!(third, first);
    assert_ne!(third, second, "a retired lineage's name is never reused");
}

#[test]
fn an_ended_lineage_counts_until_it_is_retired() {
    let script = FaultScript::new().on(When::any().op(ScriptOp::Commit), Fault::Hold);
    let mut rig = scripted_rig(declared(|c| c.lineages = 2), script);
    let minted = rig.core.mint_lineage(None).expect("room");
    rig.write_with(KEY, Epoch::new(minted, 1), 0, b"x", Default::default())
        .expect("accepted");
    rig.run();
    rig.seal_in(minted, 1);
    rig.run();
    let actions = rig.core.end_lineage(minted).expect("live");
    rig.act(actions);
    assert_eq!(
        rig.core.mint_lineage(None).err(),
        Some(at(Limit::Lineages, 2)),
        "ended, with its commit out: not retired"
    );
    rig.backend.release_all();
    rig.run();
    rig.core
        .mint_lineage(None)
        .expect("the commit answered and the lineage retired");
}

#[test]
fn an_import_past_the_imports_declared_is_refused_and_a_resolution_lifts_it() {
    let mut rig = rig_with(declared(|c| c.imports = 2));
    rig.write(1, 0, b"x").expect("room");
    rig.run();
    rig.entries();
    let mut made = Vec::new();
    for _ in 0..2 {
        let (_, actions) = rig.core.import_failure(ImportScope::All).expect("room");
        rig.act(actions);
        made.extend(tokens(&mut rig));
    }
    let refused = rig.core.import_failure(ImportScope::All);
    assert!(matches!(
        refused,
        Err(ImportError::AtCapacity(AtCapacity {
            limit: Limit::Imports,
            declared: 2
        }))
    ));
    assert_eq!(rig.core.failures().len(), 2, "nothing was recorded");
    let token = made.remove(0);
    let actions = rig
        .core
        .resolve(vec![(token, Resolution::Abandon)])
        .expect("resolved");
    rig.act(actions);
    assert!(
        matches!(
            rig.core.import_failure(ImportScope::All),
            Err(ImportError::AtCapacity(_))
        ),
        "resolved, but its Abandoned not yet popped (DI-D-108)"
    );
    rig.entries();
    let (_, actions) = rig
        .core
        .import_failure(ImportScope::All)
        .expect("a place was let go");
    rig.act(actions);
}

#[test]
fn a_failure_that_was_not_imported_is_not_counted_against_the_imports() {
    let script = FaultScript::new().on(
        When::any().op(ScriptOp::Commit),
        Fault::Fail(crate::memory::scripted_fault("the flush failed")),
    );
    let mut rig = scripted_rig(declared(|c| c.imports = 1), script);
    rig.write(1, 0, b"x").expect("room");
    rig.seal(1);
    rig.run();
    assert!(matches!(
        rig.core.failures()[0].cause,
        FailureCause::Commit { .. }
    ));
    assert!(
        rig.core.import_failure(ImportScope::All).is_ok(),
        "the commit's failure is not an import"
    );
}

#[test]
fn an_import_naming_an_unknown_lineage_is_refused_as_that_even_at_capacity() {
    let mut rig = rig_with(declared(|c| c.imports = 0));
    let other = core_with(CAPACITY);
    let refused = rig
        .core
        .import_failure(ImportScope::Lineage(other.default_lineage()));
    assert!(matches!(refused, Err(ImportError::UnknownLineage(_))));
    assert!(matches!(
        rig.core.import_failure(ImportScope::All),
        Err(ImportError::AtCapacity(_))
    ));
}

#[test]
fn an_add_target_past_the_targets_declared_is_refused_and_a_remove_lifts_it() {
    let mut core = core_with(declared(|c| c.targets = 2));
    core.add_target(KEY, declaring(&["d1"])).expect("room");
    core.add_target(OTHER, declaring(&["d2"])).expect("room");
    let third = IoTargetKey(3);
    let refused = core
        .add_target(third, declaring(&["d3"]))
        .expect_err("full");
    assert_eq!(
        refused.cause,
        AddTargetCause::AtCapacity(at(Limit::Targets, 2))
    );
    assert_eq!(refused.key, third);
    assert!(core.target(third).is_none(), "nothing was added");
    core.remove_target(OTHER).expect("idle");
    core.add_target(third, refused.target)
        .expect("a place was let go, and the description came back usable");
}

#[test]
fn a_key_in_use_is_refused_as_that_even_at_capacity() {
    let mut core = core_with(declared(|c| c.targets = 1));
    core.add_target(KEY, declaring(&[])).expect("room");
    let refused = core.add_target(KEY, declaring(&[])).expect_err("in use");
    assert_eq!(refused.cause, AddTargetCause::KeyInUse);
}

#[test]
fn a_target_taking_the_core_past_the_domains_declared_is_refused_and_interns_none() {
    let mut core = core_with(declared(|c| c.domains = 2));
    core.add_target(KEY, declaring(&["x"])).expect("one of two");
    let refused = core
        .add_target(OTHER, declaring(&["y", "z"]))
        .expect_err("two new, one left");
    assert_eq!(
        refused.cause,
        AddTargetCause::AtCapacity(at(Limit::Domains, 2))
    );
    core.add_target(OTHER, declaring(&["y"]))
        .expect("the refusal interned neither y nor z");
}

#[test]
fn a_target_reusing_the_domains_the_core_knows_is_accepted_at_capacity() {
    let mut core = core_with(declared(|c| c.domains = 1));
    core.add_target(KEY, declaring(&["x"])).expect("one of one");
    core.add_target(OTHER, declaring(&["x"]))
        .expect("known, so not counted again");
}

#[test]
fn a_domain_a_target_repeats_counts_once() {
    let mut core = core_with(declared(|c| c.domains = 1));
    core.add_target(KEY, declaring(&["x", "x"]))
        .expect("one distinct");
}

#[test]
fn a_domain_stays_counted_after_its_target_is_removed() {
    let mut core = core_with(declared(|c| c.domains = 1));
    core.add_target(KEY, declaring(&["x"])).expect("room");
    core.remove_target(KEY).expect("idle");
    let refused = core
        .add_target(KEY, declaring(&["y"]))
        .expect_err("x is kept");
    assert_eq!(
        refused.cause,
        AddTargetCause::AtCapacity(at(Limit::Domains, 1))
    );
    core.add_target(KEY, declaring(&["x"])).expect("x is known");
}

#[test]
fn the_providers_domains_count_against_the_domains_declared() {
    let one = declared(|c| c.domains = 1);
    let mut core = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        one,
        vec![domain("p")],
    )
    .expect("one of one");
    assert!(core.add_target(KEY, declaring(&["q"])).is_err());
    core.add_target(KEY, declaring(&["p"]))
        .expect("the provider's own");
}
