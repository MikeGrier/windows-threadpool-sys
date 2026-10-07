// Copyright (c) 2026 Mike Grier

use std::cmp::Ordering;
use std::collections::HashSet;
use std::sync::Arc;

use crate::ids::{DioringIds, InstanceId, Lineage, OpId};
use crate::types::{
    Epoch, FileBusy, FileKey, FileOptions, FlushDomain, ReadOptions, SuspectSet, SuspectWrite,
    WriteCaching, WriteOptions,
};

type V = DioringIds<u64>;

fn lineage(instance: InstanceId, seq: u64) -> Lineage {
    Lineage { instance, seq }
}

#[test]
fn epochs_of_one_lineage_order_by_id() {
    let l = lineage(InstanceId::next(), 0);
    let low = Epoch::<V>::new(l, 4);
    let high = Epoch::<V>::new(l, 9);
    assert_eq!(low.partial_cmp(&high), Some(Ordering::Less));
    assert_eq!(high.partial_cmp(&low), Some(Ordering::Greater));
    assert_eq!(low.partial_cmp(&low), Some(Ordering::Equal));
    assert!(low < high);
}

#[test]
fn epochs_of_different_lineages_are_unordered_and_unequal() {
    let instance = InstanceId::next();
    let a = Epoch::<V>::new(lineage(instance, 0), 4);
    let b = Epoch::<V>::new(lineage(instance, 1), 4);
    assert_eq!(a.partial_cmp(&b), None);
    assert_ne!(a, b, "the same id in two lineages names two epochs");
}

#[test]
fn the_same_lineage_seq_on_two_instances_is_two_lineages() {
    let a = Epoch::<V>::new(lineage(InstanceId::next(), 0), 1);
    let b = Epoch::<V>::new(lineage(InstanceId::next(), 0), 1);
    assert_ne!(a, b);
    assert_eq!(a.partial_cmp(&b), None);
}

#[test]
fn a_flush_domain_is_its_bytes() {
    let a = FlushDomain::new(b"disk-A".to_vec());
    let b = FlushDomain::new(&b"disk-A"[..]);
    let c = FlushDomain::new(b"disk-B".to_vec());
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.bytes(), b"disk-A");
    let set: HashSet<_> = [a, b, c].into_iter().collect();
    assert_eq!(set.len(), 2, "equal bytes hash alike");
}

#[test]
fn an_empty_flush_domain_is_a_domain() {
    let empty = FlushDomain::new(Vec::new());
    assert_eq!(empty.bytes(), b"");
    assert_ne!(empty, FlushDomain::new(b"x".to_vec()));
}

#[test]
fn file_options_declare_nothing_until_told() {
    assert!(FileOptions::new().domains.is_empty());
    assert!(FileOptions::default().domains.is_empty());
    let declared = FileOptions::new().domains([FlushDomain::new(b"A".to_vec())]);
    assert_eq!(declared.domains, vec![FlushDomain::new(b"A".to_vec())]);
}

#[test]
fn declaring_domains_again_replaces_them() {
    let options = FileOptions::new()
        .domains([FlushDomain::new(b"A".to_vec())])
        .domains([
            FlushDomain::new(b"B".to_vec()),
            FlushDomain::new(b"C".to_vec()),
        ]);
    assert_eq!(
        options.domains,
        vec![
            FlushDomain::new(b"B".to_vec()),
            FlushDomain::new(b"C".to_vec())
        ]
    );
}

#[test]
fn write_options_default_to_no_gate_and_cached() {
    for options in [WriteOptions::<V>::new(), WriteOptions::<V>::default()] {
        assert!(options.gate.is_none());
        assert_eq!(options.caching, WriteCaching::Cached);
    }
    assert_eq!(WriteCaching::default(), WriteCaching::Cached);
}

#[test]
fn write_option_setters_set_and_the_last_gate_wins() {
    let l = lineage(InstanceId::next(), 0);
    let options = WriteOptions::<V>::new()
        .gate(Epoch::new(l, 1))
        .caching(WriteCaching::WriteThrough)
        .gate(Epoch::new(l, 2));
    assert_eq!(options.gate, Some(Epoch::new(l, 2)));
    assert_eq!(options.caching, WriteCaching::WriteThrough);
}

#[test]
fn read_options_default_to_no_gate() {
    let l = lineage(InstanceId::next(), 0);
    assert!(ReadOptions::<V>::new().gate.is_none());
    assert!(ReadOptions::<V>::default().gate.is_none());
    assert_eq!(
        ReadOptions::<V>::new().gate(Epoch::new(l, 7)).gate,
        Some(Epoch::new(l, 7))
    );
}

#[test]
fn a_suspect_set_is_shared_not_copied() {
    let instance = InstanceId::next();
    let l = lineage(instance, 0);
    let writes: Arc<[SuspectWrite<V>]> = vec![
        SuspectWrite {
            op: OpId { instance, seq: 3 },
            file: FileKey(1),
            epoch: Epoch::new(l, 41),
        },
        SuspectWrite {
            op: OpId { instance, seq: 4 },
            file: FileKey(2),
            epoch: Epoch::new(l, 43),
        },
    ]
    .into();
    let set = SuspectSet(writes);
    let copy = set.clone();
    assert_eq!(set.writes().len(), 2);
    assert_eq!(set.writes()[1].epoch, Epoch::new(l, 43));
    assert!(std::ptr::eq(set.writes(), copy.writes()));
}

fn busy() -> FileBusy<V> {
    FileBusy {
        file: FileKey(9),
        in_flight: 0,
        held_for_gate: 0,
        lowest_gates: Vec::new(),
        uncovered: 0,
        uncovered_through: Vec::new(),
        failures: Vec::new(),
    }
}

#[test]
fn file_busy_names_the_file_and_nothing_it_was_not_given() {
    assert_eq!(busy().to_string(), "file FileKey(9) is still in use:");
}

#[test]
fn file_busy_says_what_clears_each_hold_it_reports() {
    let instance = InstanceId::next();
    let l = lineage(instance, 0);
    let in_flight = FileBusy {
        in_flight: 2,
        ..busy()
    }
    .to_string();
    assert!(
        in_flight.contains("2 operation(s) in flight, pop their completions"),
        "{in_flight}"
    );

    let gated = FileBusy {
        held_for_gate: 3,
        lowest_gates: vec![Epoch::new(l, 5)],
        ..busy()
    }
    .to_string();
    assert!(gated.contains("3 held until each of"), "{gated}");
    assert!(gated.contains("is durable or abandoned"), "{gated}");

    let uncovered = FileBusy {
        uncovered: 4,
        uncovered_through: vec![Epoch::new(l, 6)],
        ..busy()
    }
    .to_string();
    assert!(
        uncovered.contains("4 write(s) not yet covered, seal through each of"),
        "{uncovered}"
    );

    let failed = FileBusy {
        failures: vec![crate::ids::FailureId { instance, seq: 1 }],
        ..busy()
    }
    .to_string();
    assert!(failed.contains("heal or abandon them"), "{failed}");
}

#[test]
fn file_busy_reports_every_hold_at_once() {
    let instance = InstanceId::next();
    let l = lineage(instance, 0);
    let all = FileBusy {
        in_flight: 1,
        held_for_gate: 1,
        lowest_gates: vec![Epoch::new(l, 1)],
        uncovered: 1,
        uncovered_through: vec![Epoch::new(l, 2)],
        failures: vec![crate::ids::FailureId { instance, seq: 1 }],
        ..busy()
    }
    .to_string();
    for part in [
        "in flight",
        "held until",
        "not yet covered",
        "heal or abandon",
    ] {
        assert!(all.contains(part), "missing {part:?} in {all}");
    }
}
