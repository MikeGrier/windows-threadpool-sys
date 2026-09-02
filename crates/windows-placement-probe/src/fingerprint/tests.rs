// Copyright (c) Mike Grier.

//! Tests for the fingerprint's rendering.
//!
//! The rendering is what makes two runs comparable, so it is what is tested:
//! a format that changes shape between hosts, or that renders two different
//! machines identically, would silently defeat the whole point.
//!
//! Discovery itself is not tested here -- it reads the real machine, so an
//! assertion about its output would be an assertion about whatever hardware
//! happens to run the suite.

use windows_topology_sys::Provenance;

use super::Fingerprint;

/// The development host: 12 cores, no SMT, two L2 domains, two efficiency
/// classes, one NUMA node.
fn arm64_dev_host() -> Fingerprint {
    Fingerprint {
        arch: "aarch64",
        processors: 12,
        cores: 12,
        smt: false,
        partitioning_cache_level: Some(2),
        cache_domain_sizes: vec![6, 6],
        efficiency_classes: vec![(0, 6), (1, 6)],
        numa_node_sizes: vec![12],
        // These fixtures stand in for real hosts, so they render as real hosts
        // do -- without a taint prefix. The exact-string assertions below are
        // assertions about what those machines actually print.
        provenance: Provenance::Measured,
    }
}

/// The shape an 8C/16T homogeneous x64 host is expected to take.
fn x64_smt_host() -> Fingerprint {
    Fingerprint {
        arch: "x86_64",
        processors: 16,
        cores: 8,
        smt: true,
        partitioning_cache_level: Some(3),
        cache_domain_sizes: vec![16],
        efficiency_classes: vec![(0, 16)],
        numa_node_sizes: vec![16],
        provenance: Provenance::Measured,
    }
}

#[test]
fn the_development_host_renders_as_expected() {
    assert_eq!(
        arm64_dev_host().to_string(),
        "aarch64 12p/12c smt- L2[6,6] ec[0:6,1:6] numa[12]"
    );
}

#[test]
fn an_smt_host_renders_as_expected() {
    assert_eq!(
        x64_smt_host().to_string(),
        "x86_64 16p/8c smt+ L3[16] ec[0:16] numa[16]"
    );
}

#[test]
fn smt_is_stated_rather_than_left_to_be_inferred() {
    // `16p/8c` already implies SMT arithmetically, but the marker is what a
    // reader scans for, and the arithmetic does not survive a machine with
    // offline processors.
    assert!(x64_smt_host().to_string().contains("smt+"));
    assert!(arm64_dev_host().to_string().contains("smt-"));
}

#[test]
fn the_two_hosts_do_not_render_identically() {
    assert_ne!(
        arm64_dev_host().to_string(),
        x64_smt_host().to_string(),
        "two machines that express different placements must be distinguishable"
    );
}

#[test]
fn a_machine_no_cache_partitions_says_so() {
    let flat = Fingerprint {
        arch: "x86_64",
        processors: 4,
        cores: 4,
        smt: false,
        partitioning_cache_level: None,
        cache_domain_sizes: vec![4],
        efficiency_classes: vec![(0, 4)],
        numa_node_sizes: vec![4],
        provenance: Provenance::Measured,
    };
    assert!(
        flat.to_string().contains("L-[4]"),
        "an undivided machine must render distinctly from a divided one, got {flat}"
    );
    assert!(!flat.partitioned());
}

#[test]
fn heterogeneity_is_reported_from_the_classes() {
    assert!(arm64_dev_host().heterogeneous());
    assert!(!x64_smt_host().heterogeneous());
}

#[test]
fn partitioning_is_reported_from_the_domain_count() {
    assert!(arm64_dev_host().partitioned());
    assert!(
        !x64_smt_host().partitioned(),
        "one cache domain covering everything partitions nothing"
    );
}

#[test]
fn rendering_is_stable_across_calls() {
    // Canonical output is what lets two runs be compared by string equality,
    // so nothing in the rendering may depend on iteration order or on time.
    let host = arm64_dev_host();
    assert_eq!(host.to_string(), host.to_string());
}

#[test]
fn every_field_that_changes_the_answer_appears_in_the_render() {
    // A fingerprint that omitted a field would render two genuinely different
    // machines identically, which is worse than having no fingerprint: it
    // would license a comparison that is not valid.
    let base = arm64_dev_host();

    let mut fewer_cores = base.clone();
    fewer_cores.cores = 6;
    fewer_cores.processors = 6;

    let mut with_smt = base.clone();
    with_smt.smt = true;

    let mut different_cache = base.clone();
    different_cache.cache_domain_sizes = vec![4, 8];

    let mut different_classes = base.clone();
    different_classes.efficiency_classes = vec![(0, 12)];

    let mut different_numa = base.clone();
    different_numa.numa_node_sizes = vec![6, 6];

    let mut different_level = base.clone();
    different_level.partitioning_cache_level = Some(3);

    for (name, variant) in [
        ("core count", fewer_cores),
        ("smt", with_smt),
        ("cache domains", different_cache),
        ("efficiency classes", different_classes),
        ("numa nodes", different_numa),
        ("cache level", different_level),
    ] {
        assert_ne!(
            base.to_string(),
            variant.to_string(),
            "changing {name} must change the fingerprint"
        );
    }
}

#[test]
fn a_measured_host_renders_without_any_marker() {
    // Every fingerprint recorded in a checklist or design note before the
    // marker existed was measured, so this is what keeps those strings valid
    // and comparable rather than silently reinterpreted.
    let rendered = arm64_dev_host().to_string();

    assert!(!rendered.contains("!!"), "got {rendered}");
    assert!(rendered.starts_with("aarch64"), "got {rendered}");
}

#[test]
fn a_synthetic_host_is_marked_at_the_front() {
    let mut fabricated = x64_smt_host();
    fabricated.provenance = Provenance::Synthetic;

    let rendered = fabricated.to_string();

    assert!(
        rendered.starts_with("!!SYNTHETIC!! "),
        "the marker must lead, so a reader scanning a column cannot skip it: {rendered}"
    );
}

#[test]
fn a_restored_host_is_marked_and_says_which_kind_of_untrusted_it_is() {
    // Restored and synthetic are different claims -- one describes some real
    // machine, the other describes none -- and a reader deciding how much to
    // believe a number needs to know which.
    let mut loaded = x64_smt_host();
    loaded.provenance = Provenance::Restored;

    let rendered = loaded.to_string();

    assert!(rendered.starts_with("!!RESTORED!! "), "got {rendered}");
    assert!(!rendered.contains("SYNTHETIC"), "got {rendered}");
}

#[test]
fn an_untrusted_host_never_compares_equal_to_the_real_one_it_imitates() {
    // The specific bug the marker exists to prevent, and the reason it lives
    // inside the string rather than beside it. The fingerprint is documented as
    // canonical, so equality of the rendered form is a supported comparison --
    // which means a fabricated machine claiming the exact shape of a real one
    // must not produce the same string.
    let real = x64_smt_host();
    for untrusted in [Provenance::Synthetic, Provenance::Restored] {
        let mut imitation = x64_smt_host();
        imitation.provenance = untrusted;

        assert_eq!(
            imitation.processors, real.processors,
            "the fixtures must otherwise be identical for this test to mean anything"
        );
        assert_ne!(
            imitation.to_string(),
            real.to_string(),
            "{untrusted:?} rendered identically to a measured host"
        );
    }
}

#[test]
fn the_marker_is_the_only_difference_an_untrusted_host_renders() {
    // The taint must not disturb the shape it prefixes, or a tainted
    // fingerprint could not be compared against a real one at all -- which is
    // exactly what someone validating synthetic selection logic needs to do.
    let real = x64_smt_host();
    let mut fabricated = x64_smt_host();
    fabricated.provenance = Provenance::Synthetic;

    let rendered = fabricated.to_string();
    let stripped = rendered
        .strip_prefix("!!SYNTHETIC!! ")
        .expect("the marker must be a removable prefix");

    assert_eq!(stripped, real.to_string());
}

#[test]
fn a_fingerprint_read_from_this_machine_reports_itself_as_measured() {
    // Ties the rendering to the real path: `discover` goes through
    // `Topology::discover`, which is the only thing entitled to claim the
    // machine. If provenance ever stopped flowing, every probe banner would
    // quietly start printing a taint marker -- or worse, stop printing one.
    let fingerprint = Fingerprint::discover().expect("this machine must be discoverable");

    assert!(fingerprint.provenance.is_measured());
    assert!(!fingerprint.to_string().contains("!!"));
}

#[test]
fn a_fingerprint_built_from_a_hand_made_topology_is_not_measured() {
    // The path a synthetic host takes. `Topology::default` is untrusted by
    // construction, and `from_topology` must carry that through rather than
    // inventing an answer.
    let fingerprint = Fingerprint::from_topology(&windows_topology_sys::Topology::default());

    assert!(!fingerprint.provenance.is_measured());
    assert!(
        fingerprint.to_string().starts_with("!!SYNTHETIC!! "),
        "got {fingerprint}"
    );
}

#[test]
fn the_banner_carries_whatever_the_fingerprint_says() {
    // On this machine the topology is real, so the banner must be clean. The
    // point is that the banner is not a second, independent rendering that
    // could drift from the fingerprint's own.
    let banner = super::banner_line();
    let fingerprint = Fingerprint::discover().expect("this machine must be discoverable");

    assert!(banner.starts_with("host:  "), "got {banner}");
    assert!(
        banner.contains(&fingerprint.to_string()),
        "the banner must embed the fingerprint verbatim: {banner}"
    );
    assert!(
        !banner.contains("!!"),
        "a real machine's banner must carry no taint marker: {banner}"
    );
}

// ---------------------------------------------------------------------------
// Converting a whole topology into processor positions.
//
// `places_from_topology` carries real rules -- which cache level partitions the
// machine, which core and efficiency class each processor belongs to, and which
// NUMA node -- and until it gained a seam none of them could be exercised
// against anything but whatever machine ran the suite. The NUMA lookup was the
// worst case: on a single-node host a completely broken map and a correct one
// both yield node 0, so it was shipped unverified.
// ---------------------------------------------------------------------------

mod from_topology {
    use windows_topology_sys::{
        Domain, DomainKind, Processor, ProcessorId, ProcessorSet, Topology,
    };

    use crate::fingerprint::places_from_topology;

    /// How many processors each core carries, and where each core sits.
    struct CoreSpec {
        efficiency_class: u8,
        cache_domain: u32,
        numa_node: u32,
        threads: u8,
    }

    /// Assemble a topology from a list of cores, the way Windows would report
    /// one: a group domain, a core domain per core, a cache domain per distinct
    /// cache id, and a memory domain per distinct node.
    fn topology_of(cores: &[CoreSpec]) -> Topology {
        let mut processors = Vec::new();
        let mut domains = Vec::new();
        let mut next_number = 0_u8;
        let mut cache_members: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut node_members: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut all = Vec::new();

        for (index, core) in cores.iter().enumerate() {
            let mut members = Vec::new();
            for _ in 0..core.threads {
                processors.push(Processor {
                    id: ProcessorId {
                        group: 0,
                        number: next_number,
                    },
                    online: true,
                    capacity: 0,
                });
                members.push(next_number);
                all.push(next_number);
                next_number += 1;
            }

            domains.push(Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: core.threads > 1,
                    efficiency_class: core.efficiency_class,
                },
                id: index as u32,
                processors: set_of(&members),
            });

            push_members(&mut cache_members, core.cache_domain, &members);
            push_members(&mut node_members, core.numa_node, &members);
        }

        domains.insert(
            0,
            Domain {
                kind: DomainKind::Group,
                id: 0,
                processors: set_of(&all),
            },
        );

        for (id, members) in cache_members {
            domains.push(Domain {
                kind: DomainKind::Cache {
                    level: 2,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 512 * 1024,
                    cache_type: windows_topology_sys::CacheKind::Unified,
                },
                id,
                processors: set_of(&members),
            });
        }
        for (id, members) in node_members {
            domains.push(Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id,
                processors: set_of(&members),
            });
        }

        Topology {
            processors,
            domains,
            distances: None,
            ..Default::default()
        }
    }

    fn set_of(numbers: &[u8]) -> ProcessorSet {
        let mask = numbers.iter().fold(0_usize, |mask, n| mask | (1 << n));
        ProcessorSet::from_group_mask(0, mask)
    }

    fn push_members(into: &mut Vec<(u32, Vec<u8>)>, id: u32, members: &[u8]) {
        match into.iter_mut().find(|(existing, _)| *existing == id) {
            Some((_, list)) => list.extend_from_slice(members),
            None => into.push((id, members.to_vec())),
        }
    }

    /// Two nodes, two cores each, two threads per core.
    fn two_node_host() -> Topology {
        topology_of(&[
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 0,
                numa_node: 0,
                threads: 2,
            },
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 1,
                numa_node: 0,
                threads: 2,
            },
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 2,
                numa_node: 1,
                threads: 2,
            },
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 3,
                numa_node: 1,
                threads: 2,
            },
        ])
    }

    #[test]
    fn every_processor_is_placed() {
        let places =
            places_from_topology(&two_node_host()).expect("the fixture places every processor");

        assert_eq!(places.len(), 8);
        let mut numbers: Vec<u8> = places.iter().map(|p| p.number).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, (0..8).collect::<Vec<u8>>());
    }

    #[test]
    fn numa_nodes_are_read_from_the_memory_domains() {
        // The assertion that could not be made before this seam existed. On a
        // single-node host this passes whether the lookup works or returns the
        // fallback, so it was previously untested in the only way that matters.
        let places =
            places_from_topology(&two_node_host()).expect("the fixture places every processor");

        for place in &places {
            let expected = u32::from(place.number >= 4);
            assert_eq!(
                place.numa_node, expected,
                "cpu{} landed on node {} rather than {expected}",
                place.number, place.numa_node
            );
        }
    }

    #[test]
    fn both_nodes_are_actually_represented() {
        // Guards the degenerate pass: if the lookup silently returned 0 for
        // everything, the test above would still fail, but a future refactor
        // that collapsed the map could otherwise leave a suite that only ever
        // sees one node.
        let places =
            places_from_topology(&two_node_host()).expect("the fixture places every processor");
        let mut nodes: Vec<u32> = places.iter().map(|p| p.numa_node).collect();
        nodes.sort_unstable();
        nodes.dedup();

        assert_eq!(nodes, vec![0, 1]);
    }

    #[test]
    fn smt_siblings_share_a_core_id() {
        let places =
            places_from_topology(&two_node_host()).expect("the fixture places every processor");

        for pair in places.chunks(2) {
            assert_eq!(
                pair[0].core, pair[1].core,
                "cpu{} and cpu{} were reported on different cores",
                pair[0].number, pair[1].number
            );
        }
        assert_ne!(places[0].core, places[2].core);
    }

    #[test]
    fn the_partitioning_cache_level_is_the_outermost_one_that_divides() {
        // Four distinct L2 domains here, so every core sits behind its own and
        // the two siblings of a core share one.
        let places =
            places_from_topology(&two_node_host()).expect("the fixture places every processor");

        assert_eq!(places[0].cache_domain, places[1].cache_domain);
        assert_ne!(places[0].cache_domain, places[2].cache_domain);
    }

    #[test]
    fn a_single_cache_domain_partitions_nothing() {
        // One cache covering the whole machine divides it into one piece, which
        // is no division at all -- the rule the real hosts exercise from the
        // other side, since one reports a single L3 and falls back to L2.
        let flat = topology_of(&[
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 0,
                numa_node: 0,
                threads: 1,
            },
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 0,
                numa_node: 0,
                threads: 1,
            },
        ]);

        let places = places_from_topology(&flat).expect("the fixture places every processor");

        assert!(
            places.iter().all(|p| p.cache_domain.is_none()),
            "an undivided machine reported a partitioning cache domain"
        );
    }

    #[test]
    fn efficiency_classes_are_carried_through() {
        let hybrid = topology_of(&[
            CoreSpec {
                efficiency_class: 1,
                cache_domain: 0,
                numa_node: 0,
                threads: 2,
            },
            CoreSpec {
                efficiency_class: 0,
                cache_domain: 1,
                numa_node: 0,
                threads: 1,
            },
        ]);

        let places = places_from_topology(&hybrid).expect("the fixture places every processor");

        assert_eq!(places[0].efficiency_class, 1);
        assert_eq!(places[1].efficiency_class, 1);
        assert_eq!(places[2].efficiency_class, 0);
    }

    #[test]
    fn a_synthetic_topology_drives_the_classifier_end_to_end() {
        // The whole point of routing through `Topology`: selection now runs on
        // positions the real conversion produced, not on positions a test
        // author assumed it would produce.
        use crate::core_affinity::{Placement, node_pairs, representative_pairs};

        let places =
            places_from_topology(&two_node_host()).expect("the fixture places every processor");
        let pairs = representative_pairs(&places);

        assert!(pairs.contains_key(&Placement::SameCoreSiblings));
        assert!(pairs.contains_key(&Placement::CrossCacheSameClass));
        assert!(pairs.contains_key(&Placement::CrossNumaNode));

        // Both directions, from positions the real conversion produced: the
        // edge is one link but two measurements, because the producer writes
        // and the consumer reads.
        let hops = node_pairs(&places);
        assert_eq!(hops.len(), 2);
        assert!(hops.contains_key(&(0, 1)));
        assert!(hops.contains_key(&(1, 0)));
    }
}

// ---------------------------------------------------------------------------
// The conversion, on a machine with more than one processor group.
//
// The tests above cover classification and selection once places exist. This
// covers the step that builds them, which is where the group was previously
// discarded outright.
// ---------------------------------------------------------------------------

mod multi_group_conversion {
    use windows_topology_sys::{
        Domain, DomainKind, Processor, ProcessorId, ProcessorSet, Topology,
    };

    use crate::fingerprint::{MissingPlacement, places_from_topology};

    /// One processor per core, four cores per group, two groups -- with the
    /// numbers overlapping, which is how Windows really presents it.
    fn two_group_topology() -> Topology {
        let mut processors = Vec::new();
        let mut domains = Vec::new();
        let mut core_id = 0_u32;

        for group in 0..2_u16 {
            let mut members = Vec::new();
            for number in 0..4_u8 {
                processors.push(Processor {
                    id: ProcessorId { group, number },
                    online: true,
                    capacity: 0,
                });
                members.push(number);

                domains.push(Domain {
                    kind: DomainKind::Core {
                        simultaneous_multithreading: false,
                        efficiency_class: 0,
                    },
                    id: core_id,
                    processors: ProcessorSet::from_group_mask(group, 1_usize << number),
                });
                core_id += 1;
            }

            let mask = members.iter().fold(0_usize, |mask, n| mask | (1 << n));
            domains.push(Domain {
                kind: DomainKind::Group,
                id: u32::from(group),
                processors: ProcessorSet::from_group_mask(group, mask),
            });
            // A cache domain per group, because a cache is never shared across
            // one, and a memory domain per group so this stays a two-node
            // machine rather than accidentally testing NUMA as well.
            domains.push(Domain {
                kind: DomainKind::Cache {
                    level: 3,
                    associativity: 16,
                    line_size: 64,
                    size_bytes: 32 * 1024 * 1024,
                    cache_type: windows_topology_sys::CacheKind::Unified,
                },
                id: 100 + u32::from(group),
                processors: ProcessorSet::from_group_mask(group, mask),
            });
            domains.push(Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id: u32::from(group),
                processors: ProcessorSet::from_group_mask(group, mask),
            });
        }

        Topology {
            processors,
            domains,
            distances: None,
            ..Default::default()
        }
    }

    #[test]
    fn every_processor_of_every_group_survives_the_conversion() {
        // The regression that matters. Keying the conversion's maps on the
        // processor number alone silently produced four places for an
        // eight-processor machine, and nothing in the output said so.
        let places = places_from_topology(&two_group_topology())
            .expect("the fixture places every processor");

        assert_eq!(
            places.len(),
            8,
            "an eight-processor two-group machine converted to {} places",
            places.len()
        );

        let ids: std::collections::BTreeSet<(u16, u8)> = places.iter().map(|p| p.id()).collect();
        assert_eq!(ids.len(), 8, "two places collided on one identity");
        assert_eq!(places.iter().filter(|p| p.group == 0).count(), 4);
        assert_eq!(places.iter().filter(|p| p.group == 1).count(), 4);
    }

    #[test]
    fn a_cores_identity_does_not_collide_across_groups() {
        let places = places_from_topology(&two_group_topology())
            .expect("the fixture places every processor");

        let cores: std::collections::BTreeSet<(u16, u32)> =
            places.iter().map(|p| (p.group, p.core)).collect();

        assert_eq!(cores.len(), 8, "eight single-threaded cores collapsed");
    }

    #[test]
    fn per_group_cache_and_node_membership_is_read_correctly() {
        let places = places_from_topology(&two_group_topology())
            .expect("the fixture places every processor");

        for place in &places {
            assert_eq!(
                place.numa_node,
                u32::from(place.group),
                "{place} was read onto the wrong node"
            );
            assert_eq!(
                place.cache_domain,
                Some(100 + u32::from(place.group)),
                "{place} was read into the wrong cache domain"
            );
        }
    }

    // --- Partial topologies, which this seam exists to accept (D-12) ---

    /// A topology whose only domain is the group: online processors, no core,
    /// no cache, and no memory domain at all.
    fn bare_processors(count: u8) -> Topology {
        let all: Vec<u8> = (0..count).collect();
        let mask = all.iter().fold(0_usize, |mask, n| mask | (1 << n));
        Topology {
            processors: all
                .iter()
                .map(|&number| Processor {
                    id: ProcessorId { group: 0, number },
                    online: true,
                    capacity: 0,
                })
                .collect(),
            domains: vec![Domain {
                kind: DomainKind::Group,
                id: 0,
                processors: ProcessorSet::from_group_mask(0, mask),
            }],
            distances: None,
            ..Default::default()
        }
    }

    #[test]
    fn a_processor_with_no_core_domain_is_still_placed() {
        // The conversion used to iterate the core domains, so a processor no
        // core mentioned simply vanished -- the result described a smaller
        // machine than the topology did, and said nothing about the omission.
        let places = places_from_topology(&bare_processors(4))
            .expect("no memory domain at all means the single-node default applies");

        assert_eq!(places.len(), 4, "every online processor must be placed");
        let mut numbers: Vec<u8> = places.iter().map(|p| p.number).collect();
        numbers.sort_unstable();
        assert_eq!(numbers, vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_processor_with_no_core_domain_keeps_its_group_distinct() {
        // The core fallback was unreachable while the iteration was over core
        // domains: no core meant no entry to fall back *from*. It exists so
        // group 1's cpu5 cannot collapse onto group 0's, so that is what is
        // asserted rather than merely that some number was produced.
        let mut topology = bare_processors(1);
        topology.processors.push(Processor {
            id: ProcessorId {
                group: 1,
                number: 0,
            },
            online: true,
            capacity: 0,
        });
        topology.domains.push(Domain {
            kind: DomainKind::Group,
            id: 1,
            processors: ProcessorSet::from_group_mask(1, 0b1),
        });

        let places = places_from_topology(&topology).expect("no memory domain, so node 0 applies");

        assert_eq!(places.len(), 2);
        assert_ne!(
            places[0].core, places[1].core,
            "g0/cpu0 and g1/cpu0 must not share a fabricated core id"
        );
    }

    #[test]
    fn an_offline_processor_is_not_placed() {
        // Placement is about where work can run. An offline slot exists only to
        // reserve group capacity for a processor that may be added later.
        let mut topology = bare_processors(4);
        topology.processors[2].online = false;

        let places = places_from_topology(&topology).expect("a partial topology with no nodes");

        let numbers: Vec<u8> = places.iter().map(|p| p.number).collect();
        assert_eq!(
            numbers,
            vec![0, 1, 3],
            "the offline slot must not be placed"
        );
    }

    #[test]
    fn node_zero_is_the_answer_only_when_no_memory_domain_exists() {
        // The half of the default that is correct: a topology naming no memory
        // domain describes one node, and every processor is in it.
        let places = places_from_topology(&bare_processors(2)).expect("one implicit node");

        assert!(places.iter().all(|p| p.numa_node == 0));
    }

    #[test]
    fn a_processor_outside_every_named_memory_domain_is_refused() {
        // The half that was not. This topology names node 1 and node 2, and
        // says nothing about cpu2 -- so `unwrap_or(0)` filed it under a node
        // this machine does not have, which is exactly the fabricated label the
        // crate's own seam rule forbids.
        let mut topology = bare_processors(3);
        for (id, mask) in [(1_u32, 0b001_usize), (2, 0b010)] {
            topology.domains.push(Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id,
                processors: ProcessorSet::from_group_mask(0, mask),
            });
        }

        let refused = places_from_topology(&topology)
            .expect_err("cpu2 belongs to no named node, so it cannot be placed");

        assert_eq!(refused.group, 0);
        assert_eq!(refused.number, 2);
        assert!(
            refused.to_string().contains("g0/cpu2"),
            "the message must name the processor: {refused}"
        );
    }

    #[test]
    fn a_fully_covered_multi_node_topology_is_still_accepted() {
        // The guard must refuse only what it is aimed at: with every processor
        // covered, sparse node *numbers* are a valid machine, not an error.
        let mut topology = bare_processors(2);
        for (id, mask) in [(1_u32, 0b01_usize), (2, 0b10)] {
            topology.domains.push(Domain {
                kind: DomainKind::Memory { memory_bytes: None },
                id,
                processors: ProcessorSet::from_group_mask(0, mask),
            });
        }

        let places = places_from_topology(&topology).expect("every processor has a node");

        let mut nodes: Vec<u32> = places.iter().map(|p| p.numa_node).collect();
        nodes.sort_unstable();
        assert_eq!(nodes, vec![1, 2], "the real node numbers must survive");
    }

    // --- Unknown must never masquerade as known -------------------------------
    //
    // Placing every online processor (rather than only those a core domain
    // mentions) made three fallbacks reachable that had previously been dead:
    // a synthetic core id, class zero, and a `None` cache domain. Each compares
    // *equal* to a real value, so `classify` would report a shared core, class
    // or cache the machine does not have. Each is now refused instead, and each
    // test below pins the false sharing that would otherwise be reported.

    /// `bare_processors`, plus a real core domain covering the given numbers.
    fn with_core_domain(count: u8, id: u32, members: &[u8]) -> Topology {
        let mut topology = bare_processors(count);
        let mask = members.iter().fold(0_usize, |m, n| m | (1 << n));
        topology.domains.push(Domain {
            kind: DomainKind::Core {
                simultaneous_multithreading: members.len() > 1,
                efficiency_class: 0,
            },
            id,
            processors: ProcessorSet::from_group_mask(0, mask),
        });
        topology
    }

    #[test]
    fn a_synthetic_core_id_that_would_collide_with_a_real_one_is_refused() {
        // The collision, concretely. cpu2 has no core domain, so the old
        // fallback gave it `group << 8 | number` == 2 -- and core domain id 2
        // genuinely exists here, covering cpu0 and cpu1. `classify` compares
        // group and core, so it would have called cpu0 and cpu2 SMT siblings
        // and attributed a shared L1 that does not exist.
        let topology = with_core_domain(3, 2, &[0, 1]);

        let refused = places_from_topology(&topology)
            .expect_err("cpu2 has no core, and core id 2 is already taken by a real domain");

        assert_eq!((refused.group, refused.number), (0, 2));
        assert_eq!(refused.missing, MissingPlacement::Core);
        assert!(refused.to_string().contains("g0/cpu2"), "{refused}");
    }

    #[test]
    fn a_machine_with_no_core_domains_at_all_still_gets_distinct_synthetic_cores() {
        // The other side of that rule: with no core domain anywhere there is no
        // real id to collide with, so the synthetic namespace is safe and the
        // fallback stays. It must still keep processors apart across groups,
        // which is the reason it exists.
        let mut topology = bare_processors(1);
        topology.processors.push(Processor {
            id: ProcessorId {
                group: 1,
                number: 0,
            },
            online: true,
            capacity: 0,
        });
        topology.domains.push(Domain {
            kind: DomainKind::Group,
            id: 1,
            processors: ProcessorSet::from_group_mask(1, 0b1),
        });

        let places =
            places_from_topology(&topology).expect("no core domain exists to collide with");

        assert_eq!(places.len(), 2);
        assert_ne!(
            places[0].core, places[1].core,
            "g0/cpu0 and g1/cpu0 must not share a synthetic core id"
        );
    }

    #[test]
    fn an_unknown_efficiency_class_is_not_reported_as_class_zero() {
        // Zero is a genuine Windows efficiency class, so an uncovered processor
        // defaulting to it becomes indistinguishable from a real class-0 core --
        // and `within_class_pair` would pair them as a same-class measurement.
        // The class travels with the core, so this is the same refusal.
        let topology = with_core_domain(2, 7, &[0]);

        let refused = places_from_topology(&topology)
            .expect_err("cpu1 has no core, so its class is unknown rather than zero");

        assert_eq!((refused.group, refused.number), (0, 1));
        assert_eq!(refused.missing, MissingPlacement::Core);
    }

    #[test]
    fn a_processor_omitted_from_the_partitioning_cache_level_is_refused() {
        // `None` already means "no level partitions this machine". Letting it
        // also mean "this processor was left out of the level that does" makes
        // two omitted processors compare equal, and `classify` reports them as
        // sharing a cache -- a confident same-cache row for a pair whose cache
        // membership nobody knows.
        //
        // Three processors, each its own core so the level genuinely divides
        // them, and an L2 partition covering only two.
        let mut topology = bare_processors(3);
        for (id, member) in [(10_u32, 0_u8), (11, 1), (12, 2)] {
            topology.domains.push(Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: false,
                    efficiency_class: 0,
                },
                id,
                processors: ProcessorSet::from_group_mask(0, 1 << member),
            });
        }
        for (id, mask) in [(20_u32, 0b001_usize), (21, 0b010)] {
            topology.domains.push(Domain {
                kind: DomainKind::Cache {
                    level: 2,
                    associativity: 8,
                    line_size: 64,
                    size_bytes: 1024 * 1024,
                    cache_type: windows_topology_sys::CacheKind::Unified,
                },
                id,
                processors: ProcessorSet::from_group_mask(0, mask),
            });
        }

        let refused = places_from_topology(&topology)
            .expect_err("cpu2 is in no cache domain at the partitioning level");

        assert_eq!((refused.group, refused.number), (0, 2));
        assert_eq!(refused.missing, MissingPlacement::CacheDomain);
    }

    #[test]
    fn no_partitioning_cache_at_all_is_reported_as_none_rather_than_refused() {
        // The uniform absence, which is a real answer: a machine no cache level
        // divides has `None` for every processor, and that must keep working.
        let places =
            places_from_topology(&bare_processors(2)).expect("no cache level divides this machine");

        assert!(places.iter().all(|place| place.cache_domain.is_none()));
    }
}
