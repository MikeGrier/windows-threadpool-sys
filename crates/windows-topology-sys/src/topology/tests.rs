// Copyright (c) 2026 Mike Grier
use super::*;
use crate::domain::{Domain, DomainKind};
use crate::processor_set::ProcessorSet;
use crate::relation::CacheKind;

fn synthetic() -> Topology {
    let group0 = ProcessorSet::from_group_mask(0, 0b11);
    Topology {
        processors: vec![
            Processor {
                id: ProcessorId {
                    group: 0,
                    number: 0,
                },
                online: true,
                capacity: 0,
            },
            Processor {
                id: ProcessorId {
                    group: 0,
                    number: 1,
                },
                online: true,
                capacity: 10,
            },
        ],
        domains: vec![
            Domain {
                kind: DomainKind::Group,
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Package,
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: true,
                    efficiency_class: 10,
                },
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Cache {
                    level: 3,
                    associativity: 16,
                    line_size: 64,
                    size_bytes: 32 * 1024 * 1024,
                    cache_type: CacheKind::Unified,
                },
                id: 0,
                processors: group0.clone(),
            },
            Domain {
                kind: DomainKind::Cache {
                    level: 2,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 512 * 1024,
                    cache_type: CacheKind::Unified,
                },
                id: 1,
                processors: group0,
            },
            Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id: 0,
                processors: ProcessorSet::empty(),
            },
        ],
        distances: None,
        // Named rather than defaulted, so this fixture states what it is. The
        // helper is called `synthetic` and now says so in the value too.
        provenance: Provenance::Synthetic,
    }
}

#[test]
fn groups_returns_only_group_domains() {
    assert_eq!(synthetic().groups().count(), 1);
}

#[test]
fn packages_returns_only_package_domains() {
    assert_eq!(synthetic().packages().count(), 1);
}

#[test]
fn cores_returns_only_core_domains() {
    assert_eq!(synthetic().cores().count(), 1);
}

#[test]
fn caches_returns_every_cache_regardless_of_level() {
    assert_eq!(synthetic().caches().count(), 2);
}

#[test]
fn caches_at_level_filters_to_exactly_that_level() {
    let topo = synthetic();
    assert_eq!(topo.caches_at_level(3).count(), 1);
    assert_eq!(topo.caches_at_level(2).count(), 1);
    assert_eq!(topo.caches_at_level(1).count(), 0);
}

#[test]
fn memory_domains_includes_one_with_no_processors() {
    let topo = synthetic();
    let memory: Vec<_> = topo.memory_domains().collect();
    assert_eq!(memory.len(), 1);
    assert!(memory[0].processors.is_empty());
}

#[test]
fn processor_looks_up_by_id() {
    let topo = synthetic();
    let found = topo
        .processor(ProcessorId {
            group: 0,
            number: 1,
        })
        .expect("processor exists");
    assert_eq!(found.capacity, 10);
    assert!(
        topo.processor(ProcessorId {
            group: 9,
            number: 9
        })
        .is_none()
    );
}

#[test]
fn discover_succeeds_and_every_online_processor_is_in_some_group() {
    let topo = Topology::discover().expect("discover");
    assert!(!topo.processors.is_empty());
    assert!(topo.groups().count() >= 1);

    let all_group_processors: ProcessorSet = topo
        .groups()
        .fold(ProcessorSet::empty(), |acc, g| acc.union(&g.processors));
    for processor in topo.processors.iter().filter(|p| p.online) {
        assert!(
            all_group_processors.contains(processor.id.group, processor.id.number),
            "online processor {:?} is not a member of any group's processor set",
            processor.id
        );
    }
}

#[test]
fn discover_reports_a_processor_entry_for_every_slot_up_to_each_groups_maximum() {
    let topo = Topology::discover().expect("discover");
    let relations = crate::relation::discover().expect("discover relations");
    let expected: usize = relations
        .groups
        .iter()
        .map(|g| g.maximum_processor_count as usize)
        .sum();
    assert_eq!(topo.processors.len(), expected);
}

// --- serde (M3.3) ---

#[cfg(feature = "serde")]
mod serde_tests {
    use super::super::*;

    #[test]
    fn a_discovered_topology_round_trips_through_json_except_for_its_provenance() {
        // This test used to assert the round trip was *unchanged*. That is now
        // deliberately false, and the change is the point rather than a
        // regression: a discovered topology asserts "this is the machine you
        // are on", and once written to a file it can no longer assert that.
        // Reloading yields `Restored`.
        //
        // The assertion is deliberately not weakened to "the parts I still
        // expect to match". Everything except the provenance must survive
        // verbatim, so this compares against the original with only that field
        // adjusted -- a second corruption would still fail here.
        let topology = Topology::discover().expect("discover");
        assert!(
            topology.provenance.is_measured(),
            "discover must claim the machine it read"
        );

        let json = serde_json::to_string(&topology).expect("serialize");
        let back: Topology = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(back.provenance, Provenance::Restored);
        assert_eq!(
            back,
            Topology {
                provenance: Provenance::Restored,
                ..topology
            }
        );
    }

    #[test]
    fn a_hand_written_synthetic_topology_parses() {
        let json = r#"{
            "processors": [
                {"id": {"group": 0, "number": 0}, "online": true, "capacity": 10},
                {"id": {"group": 0, "number": 1}, "online": true, "capacity": 10}
            ],
            "domains": [
                {"kind": "group", "id": 0, "processors": [{"group":0,"number":0},{"group":0,"number":1}]},
                {"kind": "memory", "id": 0, "processors": [], "memory_bytes": 68719476736}
            ],
            "distances": null
        }"#;
        let topology: Topology = serde_json::from_str(json).expect("parse");
        assert_eq!(topology.processors.len(), 2);
        assert_eq!(topology.groups().count(), 1);
        let memory: Vec<_> = topology.memory_domains().collect();
        assert_eq!(memory.len(), 1);
        assert!(memory[0].processors.is_empty());
    }

    /// A description shaped like what a Linux system would produce: a
    /// single processor group (Linux has no group concept), a memory-only
    /// node, and a populated scalar distance matrix -- all things Windows
    /// itself never reports through this crate's own discovery, but that a
    /// fed-in description can legitimately carry (D-10).
    #[test]
    fn a_linux_shaped_description_with_a_memory_only_node_and_distances_parses() {
        let json = r#"{
            "processors": [
                {"id": {"group": 0, "number": 0}, "online": true, "capacity": 1024},
                {"id": {"group": 0, "number": 1}, "online": true, "capacity": 1024}
            ],
            "domains": [
                {"kind": "group", "id": 0, "processors": [{"group":0,"number":0},{"group":0,"number":1}]},
                {"kind": "memory", "id": 0, "processors": [{"group":0,"number":0},{"group":0,"number":1}],
                 "memory_bytes": 17179869184},
                {"kind": "memory", "id": 1, "processors": [], "memory_bytes": 549755813888}
            ],
            "distances": {"over": "memory", "matrix": [[10, 40], [40, 10]]}
        }"#;
        let topology: Topology = serde_json::from_str(json).expect("parse");
        assert_eq!(topology.memory_domains().count(), 2);
        assert!(
            topology.memory_domains().any(|d| d.processors.is_empty()),
            "the CXL-shaped node must survive"
        );
        let distances = topology.distances.expect("distances present");
        assert_eq!(distances.over, "memory");
        assert_eq!(distances.matrix, vec![vec![10, 40], vec![40, 10]]);
    }

    /// The other half of D-10: a single "group" holding more than 64
    /// processors -- ordinary on Linux, where there is no group concept at
    /// all -- cannot be materialised into this crate's `ProcessorSet`, whose
    /// mask is one machine word because a real `GROUP_AFFINITY` is. The
    /// documented, sanctioned response is to reject rather than silently
    /// drop processors 64 and up.
    #[test]
    fn a_description_claiming_more_than_64_processors_in_one_group_is_rejected() {
        let json = r#"{
            "processors": [],
            "domains": [
                {"kind": "group", "id": 0, "processors": [{"group": 0, "number": 100}]}
            ],
            "distances": null
        }"#;
        let error = serde_json::from_str::<Topology>(json)
            .expect_err("processor number 100 is out of range");
        assert!(
            error.to_string().contains("100"),
            "error should name the offending number: {error}"
        );
    }
}

#[test]
fn a_hand_built_topology_is_not_measured() {
    // The fixture above names `Synthetic` explicitly; this pins down that the
    // value survives to a reader, so a consumer asking "is this my machine"
    // gets the right answer from hand-built data.
    assert_eq!(synthetic().provenance, Provenance::Synthetic);
    assert!(!synthetic().provenance.is_measured());
}

#[test]
fn a_defaulted_topology_is_not_measured() {
    // `Topology::default()` is the easiest way to obtain one and must be the
    // safe one. If this ever reports measured, every forgetful construction in
    // every dependent silently starts asserting it read the machine.
    let topology = Topology::default();

    assert_eq!(topology.provenance, Provenance::Synthetic);
    assert!(!topology.provenance.is_measured());
}

#[test]
fn struct_update_syntax_from_default_stays_untrusted() {
    // `..Default::default()` is how a caller builds a topology while naming
    // only the fields they care about, and provenance is exactly the field
    // nobody thinks to name.
    let topology = Topology {
        distances: None,
        ..Default::default()
    };

    assert!(!topology.provenance.is_measured());
}

#[cfg(feature = "serde")]
mod serde_provenance {
    use super::*;

    fn load(provenance_field: &str) -> Topology {
        let json =
            format!(r#"{{"processors": [], "domains": [], "distances": null{provenance_field}}}"#);
        serde_json::from_str(&json).expect("the description must parse")
    }

    #[test]
    fn a_description_claiming_measured_is_downgraded_to_restored() {
        // The core of the rule. A file cannot establish that it is the machine
        // you are running on, however sincerely it asserts it -- and a
        // hand-edited description is the obvious way someone would try.
        let topology = load(r#", "provenance": "measured""#);

        assert_eq!(topology.provenance, Provenance::Restored);
        assert!(!topology.provenance.is_measured());
    }

    #[test]
    fn a_description_claiming_restored_stays_restored() {
        assert_eq!(
            load(r#", "provenance": "restored""#).provenance,
            Provenance::Restored
        );
    }

    #[test]
    fn a_description_claiming_synthetic_is_not_promoted() {
        // The ceiling is a maximum, not an assignment: passing through a loader
        // must not launder fabricated data into merely-restored data.
        assert_eq!(
            load(r#", "provenance": "synthetic""#).provenance,
            Provenance::Synthetic
        );
    }

    #[test]
    fn a_description_without_the_field_loads_as_synthetic() {
        // Every description written before this field existed takes this path,
        // so the default has to be the safe one here too.
        assert_eq!(load("").provenance, Provenance::Synthetic);
    }

    #[test]
    fn a_measured_topology_does_not_survive_a_round_trip_as_measured() {
        // The property that makes persistence honest, stated end to end: you
        // may archive a real topology, and what you reload is explicitly a
        // description of a machine rather than a claim about this one.
        let mut measured = synthetic();
        measured.provenance = Provenance::Measured;

        let json = serde_json::to_string(&measured).expect("must serialize");
        assert!(
            json.contains("measured"),
            "the marker is not visible in the persisted form: {json}"
        );

        let reloaded: Topology = serde_json::from_str(&json).expect("must parse");
        assert_eq!(reloaded.provenance, Provenance::Restored);
        assert!(!reloaded.provenance.is_measured());
    }

    #[test]
    fn everything_but_the_provenance_round_trips_unchanged() {
        // The downgrade must be the *only* thing a round trip changes,
        // otherwise this would be trading one silent corruption for another.
        let mut measured = synthetic();
        measured.provenance = Provenance::Measured;

        let json = serde_json::to_string(&measured).expect("must serialize");
        let reloaded: Topology = serde_json::from_str(&json).expect("must parse");

        assert_eq!(reloaded.processors, measured.processors);
        assert_eq!(reloaded.domains, measured.domains);
        assert_eq!(reloaded.distances, measured.distances);
    }
}
