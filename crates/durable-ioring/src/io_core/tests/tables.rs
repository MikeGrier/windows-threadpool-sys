// Copyright (c) 2026 Mike Grier
//! The tables the declaration sizes (DI-D-106, DI-D-107): made when the core is built, at sizes
//! the declared counts bound, and filled to those sizes without refusing. A table that refused
//! inside its bound would be an inconsistency and a panic, so each test fills one to the bound and
//! then lets go of what filled it.

use std::sync::Arc;

use super::pairs::Checked;
use super::{Core, KEY, OTHER, Rig, Ticks, domain, nz};
use crate::capacity::{Capacity, ConstructionError};
use crate::diagnostics::{DiagnosticSink, NoDiagnostics};
use crate::memory::{Fault, FaultScript, MemoryBackend, MemoryTarget, ScriptOp, When};
use crate::provider::DomainOutcome;
use crate::test_support::CAPACITY;
use crate::types::{Entry, Epoch, ImportScope, PushRefusal, Resolution};

const WRITES: When = When::any().op(ScriptOp::Write);
const COMMITS: When = When::any().op(ScriptOp::Commit);

fn declared(f: impl FnOnce(&mut Capacity)) -> Capacity {
    let mut capacity = CAPACITY;
    f(&mut capacity);
    capacity
}

fn tight(operations: usize) -> Capacity {
    declared(|c| c.operations = operations)
}

/// A rig over `core`, with `KEY` and `OTHER` attached, over a backend doing what `script` says.
fn rig_over(core: Core, script: FaultScript) -> Rig {
    let mut rig = Rig::over(core);
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(script);
    rig.attach(KEY, MemoryTarget::new());
    rig.attach(OTHER, MemoryTarget::new());
    rig
}

fn core_with(capacity: Capacity) -> Core {
    Core::new(Ticks::default(), Arc::new(NoDiagnostics), capacity).expect("a valid capacity")
}

#[test]
fn operations_and_commits_in_flight_together_fill_the_submissions_table_and_no_further() {
    let script = FaultScript::new()
        .on(WRITES.nth(3), Fault::Hold)
        .on(WRITES.nth(4), Fault::Hold)
        .on(COMMITS, Fault::Hold);
    let mut rig = rig_over(core_with(tight(4)), script);
    rig.write_to(KEY, 1, 0, b"a").expect("room");
    rig.write_to(OTHER, 2, 0, b"b").expect("room");
    rig.run();
    rig.seal(2);
    rig.run();
    rig.write_to(KEY, 3, 1, b"c").expect("room");
    rig.write_to(OTHER, 4, 1, b"d").expect("room");
    rig.run();
    assert_eq!(rig.core.in_flight.len(), 4, "two commits and two writes");
    assert_eq!(rig.core.window_places(), 4);
    assert!(matches!(
        rig.write_to(KEY, 5, 2, b"e").expect_err("full").reason,
        PushRefusal::AtCapacity(_)
    ));
    for _ in 0..3 {
        rig.backend.release_all();
        rig.run();
    }
    rig.seal(4);
    for _ in 0..3 {
        rig.backend.release_all();
        rig.run();
    }
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
    assert!(rig.core.in_flight.is_empty());
}

#[test]
fn flush_pairs_fill_to_what_the_operations_declared_allow() {
    // Each seal's one write is to a file declared with two served domains and an unserved one, so
    // a seal opens a pair for the file's flush and one for each domain: (1 + 2) for each operation.
    let sink = Checked::pairs();
    let shown: Arc<dyn DiagnosticSink> = Arc::clone(&sink) as Arc<dyn DiagnosticSink>;
    let core = Core::with_provider(
        Ticks::default(),
        shown,
        declared(|c| {
            c.operations = 4;
            c.lineages = 8;
        }),
        vec![domain("a"), domain("b")],
    )
    .expect("a valid capacity");
    let mut rig = Rig::over(core);
    rig.backend =
        MemoryBackend::new(nz(8), nz(16)).with_script(FaultScript::new().on(COMMITS, Fault::Hold));
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains([domain("a"), domain("b"), domain("z")]),
    );
    for _ in 0..4 {
        let minted = rig.core.mint_lineage(None).expect("room");
        rig.write_with(KEY, Epoch::new(minted, 1), 0, b"x", Default::default())
            .expect("room");
        rig.run();
        rig.seal_in(minted, 1);
        rig.run();
    }
    assert_eq!(rig.core.window_places(), 4);
    assert_eq!(rig.requests.len(), 4, "a request for each seal");
    rig.backend.release_all();
    rig.run();
    for request in std::mem::take(&mut rig.requests) {
        for name in ["a", "b"] {
            let actions = rig.core.provided(
                request.lineage,
                request.seal,
                &domain(name),
                DomainOutcome::Succeeded,
            );
            rig.act(actions);
        }
    }
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
    sink.finished();
}

#[test]
fn span_entries_fill_to_the_operations_declared() {
    let capacity = declared(|c| {
        c.operations = 4;
        c.targets = 4;
    });
    let mut rig = Rig::over(core_with(capacity));
    for key in 1..=4 {
        rig.attach(crate::types::IoTargetKey(key), MemoryTarget::new());
    }
    for key in 1..=4_u64 {
        rig.write_to(crate::types::IoTargetKey(key), key, 0, b"x")
            .expect("room: a file of its own, so an entry of its own");
    }
    rig.run();
    rig.seal(2);
    rig.run();
    rig.seal(4);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn operations_in_flight_fill_the_per_target_table_to_the_targets_declared() {
    let capacity = declared(|c| {
        c.operations = 4;
        c.targets = 2;
    });
    let script = FaultScript::new().on(WRITES, Fault::Hold);
    let mut rig = rig_over(core_with(capacity), script);
    for epoch in 1..=2 {
        rig.write_to(KEY, epoch, 0, b"x").expect("room");
        rig.write_to(OTHER, epoch + 2, 0, b"y").expect("room");
    }
    rig.run();
    assert_eq!(rig.core.per_target.len(), 2, "one entry for each target");
    rig.backend.release_all();
    rig.run();
    assert!(rig.core.per_target.is_empty());
}

#[test]
fn a_declaration_too_large_for_a_table_is_refused_naming_the_table() {
    for (capacity, table) in [
        (
            declared(|c| c.operations = usize::MAX),
            "the uncovered writes",
        ),
        (declared(|c| c.lineages = usize::MAX), "the lineages"),
        // The flush-domain lists are sized by both, and are made before the targets' and the
        // domains' own tables.
        (
            declared(|c| c.targets = usize::MAX),
            "the flush-domain lists",
        ),
        (
            declared(|c| c.domains = usize::MAX),
            "the flush-domain lists",
        ),
    ] {
        let refused = Core::new(Ticks::default(), Arc::new(NoDiagnostics), capacity);
        assert!(
            matches!(refused.err(), Some(ConstructionError::Table { table: t, .. }) if t == table),
            "{table}"
        );
    }
}

#[test]
fn gated_writes_fill_the_held_tables_to_the_operations_declared_and_a_seal_releases_them() {
    let mut rig = rig_over(core_with(tight(4)), FaultScript::new());
    let gate_lineage = rig.core.mint_lineage(None).expect("room for a lineage");
    let gate = Epoch::new(gate_lineage, 1);
    for epoch in 1..=4 {
        rig.write_with(
            KEY,
            rig.epoch(epoch),
            epoch * 8,
            b"x",
            crate::types::WriteOptions::new().gate(gate),
        )
        .expect("held, and room");
    }
    assert_eq!(
        rig.core.held.len(),
        4,
        "every operation held, each at its own epoch"
    );
    assert_eq!(rig.core.window_places(), 4);
    assert!(matches!(
        rig.write_with(
            KEY,
            rig.epoch(5),
            40,
            b"y",
            crate::types::WriteOptions::new().gate(gate),
        )
        .expect_err("full")
        .reason,
        PushRefusal::AtCapacity(_)
    ));
    rig.seal_in(gate_lineage, 1);
    rig.run();
    assert_eq!(
        rig.core.held.len(),
        0,
        "the gate's empty seal released them all"
    );
    rig.seal(4);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn uncovered_writes_fill_the_write_table_to_the_operations_declared_and_a_seal_lets_them_go() {
    let mut rig = rig_over(core_with(tight(4)), FaultScript::new());
    let other = rig.core.mint_lineage(None).expect("room for a lineage");
    for epoch in 1..=2 {
        rig.write_to(KEY, epoch, epoch * 8, b"x").expect("room");
        rig.write_with(
            OTHER,
            Epoch::new(other, epoch),
            epoch * 8,
            b"y",
            Default::default(),
        )
        .expect("room");
    }
    assert_eq!(rig.core.window_places(), 4, "two lineages share one table");
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 4, "completed, and uncovered");
    assert!(matches!(
        rig.write_to(KEY, 3, 24, b"z").expect_err("full").reason,
        PushRefusal::AtCapacity(_)
    ));
    rig.seal(2);
    rig.seal_in(other, 2);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0, "covered");
    rig.write_to(KEY, 3, 24, b"z")
        .expect("room again: nothing was kept");
}

#[test]
fn held_writes_stopped_by_their_lineage_ending_let_go_of_every_table_slot() {
    let mut rig = rig_over(core_with(tight(2)), FaultScript::new());
    let gate_lineage = rig.core.mint_lineage(None).expect("room for a lineage");
    let gate = Epoch::new(gate_lineage, 1);
    let ending = rig.core.mint_lineage(None).expect("room for a lineage");
    for epoch in 1..=2 {
        rig.write_with(
            KEY,
            Epoch::new(ending, epoch),
            epoch * 8,
            b"x",
            crate::types::WriteOptions::new().gate(gate),
        )
        .expect("held, and room");
    }
    let actions = rig.core.end_lineage(ending).expect("live, not the default");
    rig.act(actions);
    rig.entries();
    assert_eq!(rig.core.held.len(), 0, "both ended as never issued");
    assert_eq!(rig.core.window_places(), 0);
    // Held again, at the same size: a slot any table kept for the stopped writes would refuse.
    for epoch in 1..=2 {
        rig.write_with(
            KEY,
            rig.epoch(epoch),
            epoch * 8,
            b"y",
            crate::types::WriteOptions::new().gate(gate),
        )
        .expect("held again, and room");
    }
    rig.seal_in(gate_lineage, 1);
    rig.run();
    rig.seal(2);
    rig.run();
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn unresolved_failures_fill_to_the_flush_parts_and_the_imports() {
    // A file declared with a served domain and an unserved one: each seal asks the default to
    // flush it and the provider for the served domain, and both parts fail -- two observed failures
    // for each operation -- and every import declared is made besides.
    let capacity = declared(|c| {
        c.operations = 4;
        c.imports = 2;
    });
    let bound = capacity.failures(1).expect("small");
    assert_eq!(bound, 10);
    let core = Core::with_provider(
        Ticks::default(),
        Arc::new(NoDiagnostics),
        capacity,
        vec![domain("a")],
    )
    .expect("a valid capacity");
    let mut rig = Rig::over(core);
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(FaultScript::new().on(
        COMMITS,
        Fault::Fail(crate::memory::scripted_fault("the flush failed")),
    ));
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains([domain("a"), domain("z")]),
    );
    for epoch in 1..=4 {
        rig.write_to(KEY, epoch, epoch, b"x").expect("room");
        rig.run();
        rig.seal(epoch);
        rig.run();
    }
    for request in std::mem::take(&mut rig.requests) {
        let failed = DomainOutcome::Failed(crate::report_io_error(std::io::Error::other("no")));
        let actions = rig
            .core
            .provided(request.lineage, request.seal, &domain("a"), failed);
        rig.act(actions);
    }
    for _ in 0..2 {
        let (_, actions) = rig
            .core
            .import_failure(crate::types::ImportScope::All)
            .expect("room");
        rig.act(actions);
    }
    assert_eq!(rig.core.failures().len(), bound, "every failure unresolved");
    let tokens: Vec<_> = rig
        .entries()
        .into_iter()
        .filter_map(|entry| match entry {
            crate::types::Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .collect();
    assert_eq!(tokens.len(), bound);
    let resolutions = tokens
        .into_iter()
        .map(|token| (token, crate::types::Resolution::Abandon))
        .collect();
    let actions = rig.core.resolve(resolutions).expect("resolved");
    rig.act(actions);
    rig.run();
    rig.entries();
    assert!(rig.core.failures().is_empty());
    assert_eq!(rig.core.window_places(), 0);

    // The token slots the resolved failures held are free again (DI-D-108): two more failures,
    // past the bound counted over the instance's life, each take one and hand out a live token.
    for _ in 0..2 {
        let (_, actions) = rig
            .core
            .import_failure(crate::types::ImportScope::All)
            .expect("room");
        rig.act(actions);
    }
    let inventory = rig.core.failures();
    assert_eq!(inventory.len(), 2);
    assert!(inventory.iter().all(|info| info.token_live));

    // A healed failure frees its slot as an abandoned one does: more heals than there are slots.
    // Nothing is uncovered, so each heal has nothing to wait for and takes effect at once.
    for _ in 0..bound {
        let heals: Vec<_> = rig
            .entries()
            .into_iter()
            .filter_map(|entry| match entry {
                crate::types::Entry::Failed(failed) => {
                    Some((failed.token, crate::types::Resolution::Heal))
                }
                _ => None,
            })
            .collect();
        for (token, _) in &heals {
            rig.oracle.healed(token.id());
        }
        let actions = rig.core.resolve(heals).expect("healed");
        rig.act(actions);
        rig.run();
        assert!(rig.core.failures().is_empty(), "each heal took effect");
        for _ in 0..2 {
            let (_, actions) = rig
                .core
                .import_failure(crate::types::ImportScope::All)
                .expect("room");
            rig.act(actions);
        }
    }
    assert_eq!(rig.core.failures().len(), 2);
}

#[test]
fn kept_writes_fill_to_the_operations_declared() {
    // A failure suspects every write declared, and one commit covers them all while it does: each
    // is kept, with its marking, until the failure is resolved.
    let mut rig = Rig::over(core_with(tight(4)));
    rig.attach(KEY, MemoryTarget::new());
    for offset in 0..4 {
        rig.write_to(KEY, 1, offset, b"x").expect("room");
    }
    rig.run();
    let (_, actions) = rig
        .core
        .import_failure(crate::types::ImportScope::All)
        .expect("room");
    rig.act(actions);
    rig.seal(1);
    rig.run();
    let token = rig
        .entries()
        .into_iter()
        .find_map(|entry| match entry {
            crate::types::Entry::Failed(failed) => Some(failed.token),
            _ => None,
        })
        .expect("the import's entry");
    let failures = rig.core.failures();
    assert_eq!(failures[0].markings.len(), 4, "each kept write's marking");
    assert_eq!(rig.core.window_places(), 4, "each kept for the failure");
    let actions = rig
        .core
        .resolve(vec![(token, crate::types::Resolution::Abandon)])
        .expect("resolved");
    rig.act(actions);
    rig.entries();
    assert_eq!(rig.core.window_places(), 0);
}

#[test]
fn domain_lists_fill_to_the_targets_and_imports_and_each_is_let_go_with_its_holder() {
    // A target declared with domains holds a list, and an import by domains another, so the rows
    // made for the targets and the imports are all held at once here. Removing a target and
    // resolving an import let theirs go: cycling both past the rows made finds one free each time,
    // where a row kept would be an inconsistency on the next round.
    let capacity = declared(|c| {
        c.targets = 2;
        c.imports = 2;
        c.domains = 3;
    });
    let mut rig = Rig::over(core_with(capacity));
    for round in 0..4 {
        rig.attach(
            KEY,
            MemoryTarget::new().with_domains([domain("a"), domain("b")]),
        );
        rig.attach(OTHER, MemoryTarget::new().with_domains([domain("c")]));
        for _ in 0..2 {
            let (_, actions) = rig
                .core
                .import_failure(ImportScope::Domains(vec![domain("b"), domain("c")]))
                .expect("room");
            rig.act(actions);
        }
        // Abandoned and healed in turn: a heal with nothing to wait for takes effect at once, and
        // either lets the import's list go.
        let resolution = if round % 2 == 0 {
            Resolution::Abandon
        } else {
            Resolution::Heal
        };
        let resolved: Vec<_> = rig
            .entries()
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Failed(failed) => Some((failed.token, resolution)),
                _ => None,
            })
            .collect();
        assert_eq!(resolved.len(), 2, "round {round}");
        if resolution == Resolution::Heal {
            for (token, _) in &resolved {
                rig.oracle.healed(token.id());
            }
        }
        let actions = rig.core.resolve(resolved).expect("resolved");
        rig.act(actions);
        rig.entries();
        for key in [KEY, OTHER] {
            rig.core.remove_target(key).expect("nothing holds it");
            crate::backend::IoBackend::detach(&mut rig.backend, key).expect("attached");
        }
    }
}

#[test]
fn a_failed_files_list_stays_the_files_once_the_failure_is_resolved() {
    // A failed flush reaches by its file's own list (DI-D-108), which the file holds: resolving the
    // failure leaves it with the file. A target added afterwards takes a row of its own, and the
    // failed file's writes still name the domains it was declared with.
    let capacity = declared(|c| {
        c.targets = 2;
        c.imports = 0;
    });
    let mut rig = Rig::over(core_with(capacity));
    rig.backend = MemoryBackend::new(nz(8), nz(16)).with_script(FaultScript::new().on(
        COMMITS.nth(1),
        Fault::Fail(crate::memory::scripted_fault("the flush failed")),
    ));
    rig.attach(
        KEY,
        MemoryTarget::new().with_domains([domain("a"), domain("b")]),
    );
    rig.write_to(KEY, 1, 1, b"x").expect("room");
    rig.run();
    rig.seal(1);
    rig.run();
    let abandons: Vec<_> = rig
        .entries()
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Failed(failed) => Some((failed.token, Resolution::Abandon)),
            _ => None,
        })
        .collect();
    assert_eq!(abandons.len(), 1, "the commit failed");
    let actions = rig.core.resolve(abandons).expect("resolved");
    rig.act(actions);
    rig.entries();

    rig.attach(OTHER, MemoryTarget::new().with_domains([domain("c")]));
    let mut declared = vec![
        rig.core.domains[&domain("a")],
        rig.core.domains[&domain("b")],
    ];
    declared.sort_unstable();
    let list = |key| {
        rig.core
            .durability
            .lists()
            .get(rig.core.targets[&key].domains)
            .to_vec()
    };
    assert_eq!(list(KEY), declared, "the failed file's list is its own");
    assert_eq!(list(OTHER), [rig.core.domains[&domain("c")]]);
    rig.write_to(KEY, 2, 2, b"y").expect("room");
    rig.run();
}
