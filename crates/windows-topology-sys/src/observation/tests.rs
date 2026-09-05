// Copyright (c) 2026 Mike Grier
//! Tests for [`Observation`] and [`Source`].

use super::{Observation, Source};

#[test]
fn a_label_is_only_meaningful_beside_its_source() {
    // The measured case: both sources name the same core partition, and the
    // numbers differ. Two observations of one relation, neither wrong.
    let walk = Observation::new(Source::RelationshipWalk, 0);
    let cpu_sets = Observation::new(Source::CpuSets, 0);

    assert_eq!(walk.label, cpu_sets.label);
    assert_ne!(
        walk, cpu_sets,
        "equal labels from different sources are different observations"
    );
}

#[test]
fn observations_from_one_source_are_distinguished_by_label() {
    assert_ne!(
        Observation::new(Source::RelationshipWalk, 0),
        Observation::new(Source::RelationshipWalk, 1)
    );
}

#[test]
fn source_is_not_a_trust_ordering() {
    // Deliberately checked: `Ord` exists so observations can be sorted into a
    // stable order, and it must not be read as "CpuSets is better than the
    // walk". Trust in the object is Provenance's job (D-22).
    let mut sources = [
        Source::CpuSets,
        Source::Description,
        Source::RelationshipWalk,
    ];
    sources.sort_unstable();
    assert_eq!(
        sources,
        [
            Source::RelationshipWalk,
            Source::CpuSets,
            Source::Description
        ],
        "sorting is declaration order, which is not a claim about authority"
    );
}

#[test]
fn a_description_names_no_platform_api() {
    // A hand-written or deserialized relation was reported by neither Win32
    // source, and saying so is different from claiming one of them.
    let described = Observation::new(Source::Description, 7);
    assert_eq!(described.source, Source::Description);
    assert_ne!(described.source, Source::RelationshipWalk);
    assert_ne!(described.source, Source::CpuSets);
}

#[cfg(feature = "serde")]
#[test]
fn an_observation_round_trips_with_its_source_intact() {
    for observation in [
        Observation::new(Source::RelationshipWalk, 0),
        Observation::new(Source::CpuSets, 14),
        Observation::new(Source::Description, u32::MAX),
    ] {
        let json = serde_json::to_string(&observation).expect("serialize");
        let back: Observation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, observation, "round trip changed {json}");
    }
}

#[cfg(feature = "serde")]
#[test]
fn the_sources_have_distinct_wire_names() {
    // A format that collapsed two sources would destroy the only thing a
    // second observer is for.
    let walk = serde_json::to_string(&Source::RelationshipWalk).expect("serialize");
    let cpu_sets = serde_json::to_string(&Source::CpuSets).expect("serialize");
    let description = serde_json::to_string(&Source::Description).expect("serialize");

    assert_ne!(walk, cpu_sets);
    assert_ne!(walk, description);
    assert_ne!(cpu_sets, description);
}
