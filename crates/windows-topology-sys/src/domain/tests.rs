// Copyright (c) 2026 Mike Grier
use std::collections::BTreeMap;

use super::*;

#[test]
fn processor_id_orders_by_group_then_number() {
    let a = ProcessorId {
        group: 0,
        number: 5,
    };
    let b = ProcessorId {
        group: 1,
        number: 0,
    };
    assert!(a < b);
}

#[test]
fn a_memory_domain_may_have_no_processors() {
    // A CXL memory expander: a memory domain that is nonetheless real and
    // must not be treated as degenerate just because it has no processors
    // (D-5). This shape is unreachable via GetLogicalProcessorInformationEx
    // on real hardware, so it is exercised here rather than through
    // discovery.
    let domain = Domain {
        kind: DomainKind::Memory {
            memory_bytes: Some(64 * 1024 * 1024 * 1024),
        },
        id: 9,
        processors: ProcessorSet::empty(),
    };
    assert!(domain.processors.is_empty());
    let DomainKind::Memory { memory_bytes } = domain.kind else {
        panic!("expected Memory")
    };
    assert_eq!(memory_bytes, Some(64 * 1024 * 1024 * 1024));
}

#[test]
fn a_discovered_memory_domain_has_no_known_size() {
    // Contrast with the test above: Windows's own enumeration cannot report
    // node memory capacity at all, so that arm must stay `None` rather than
    // guessing `Some(0)`, which would be indistinguishable from "no memory".
    let domain = Domain {
        kind: DomainKind::Memory { memory_bytes: None },
        id: 0,
        processors: ProcessorSet::empty(),
    };
    let DomainKind::Memory { memory_bytes } = domain.kind else {
        panic!("expected Memory")
    };
    assert_eq!(memory_bytes, None);
}

#[test]
fn an_unrecognised_domain_kind_carries_its_attributes() {
    let mut attributes = BTreeMap::new();
    attributes.insert("watts".to_string(), AttributeValue::Float(15.5));
    let domain = Domain {
        kind: DomainKind::Other {
            name: "power".to_string(),
            attributes: attributes.clone(),
        },
        id: 0,
        processors: ProcessorSet::empty(),
    };
    let DomainKind::Other {
        name,
        attributes: got,
    } = &domain.kind
    else {
        panic!("expected Other")
    };
    assert_eq!(name, "power");
    assert_eq!(got, &attributes);
}

#[test]
fn attribute_value_supports_nested_structures() {
    let mut inner = BTreeMap::new();
    inner.insert("a".to_string(), AttributeValue::Bool(true));
    let value = AttributeValue::Array(vec![AttributeValue::Null, AttributeValue::Object(inner)]);
    assert_eq!(
        value.clone(),
        value,
        "AttributeValue must support equality for round-trip tests later"
    );
}

#[test]
fn distances_is_expected_to_be_square() {
    let distances = Distances {
        over: "memory".to_string(),
        matrix: vec![vec![10, 21], vec![21, 10]],
    };
    assert!(
        distances
            .matrix
            .iter()
            .all(|row| row.len() == distances.matrix.len())
    );
}

// --- serde (M3.3) ---

#[cfg(feature = "serde")]
mod serde_tests {
    use std::collections::BTreeMap;

    use super::super::*;
    use crate::processor_set::ProcessorSet;

    fn round_trip(domain: &Domain) -> Domain {
        let json = serde_json::to_string(domain).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn a_group_domain_round_trips() {
        let domain = Domain {
            kind: DomainKind::Group,
            id: 0,
            processors: ProcessorSet::from_group_mask(0, 0b11),
        };
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn a_core_domain_round_trips_with_its_fields() {
        let domain = Domain {
            kind: DomainKind::Core {
                simultaneous_multithreading: true,
                efficiency_class: 7,
            },
            id: 3,
            processors: ProcessorSet::from_group_mask(0, 0b1),
        };
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn a_cache_domain_round_trips_including_its_cache_type() {
        let domain = Domain {
            kind: DomainKind::Cache {
                level: 3,
                associativity: 16,
                line_size: 64,
                size_bytes: 32 * 1024 * 1024,
                cache_type: CacheKind::Unified,
            },
            id: 0,
            processors: ProcessorSet::from_group_mask(0, 0b1111),
        };
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn a_cache_domain_with_an_unrecognised_cache_type_round_trips() {
        let domain = Domain {
            kind: DomainKind::Cache {
                level: 1,
                associativity: 8,
                line_size: 64,
                size_bytes: 32 * 1024,
                cache_type: CacheKind::Other(99),
            },
            id: 0,
            processors: ProcessorSet::from_group_mask(0, 0b1),
        };
        let json = serde_json::to_string(&domain).expect("serialize");
        assert!(
            json.contains(r#""cache_type":{"other":99}"#),
            "unexpected JSON: {json}"
        );
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn a_cache_domain_with_a_negative_unrecognised_cache_type_round_trips() {
        // PROCESSOR_CACHE_TYPE is a raw i32 enum; a value Windows reports
        // that happens to be negative must round-trip, not just the
        // non-negative range a u32-shaped decode would accept.
        let domain = Domain {
            kind: DomainKind::Cache {
                level: 1,
                associativity: 8,
                line_size: 64,
                size_bytes: 32 * 1024,
                cache_type: CacheKind::Other(-7),
            },
            id: 0,
            processors: ProcessorSet::from_group_mask(0, 0b1),
        };
        let json = serde_json::to_string(&domain).expect("serialize");
        assert!(
            json.contains(r#""cache_type":{"other":-7}"#),
            "unexpected JSON: {json}"
        );
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn a_memory_only_domain_round_trips_with_no_processors() {
        // The CXL-expander case (D-5): a memory domain with a known size and
        // no processors at all.
        let domain = Domain {
            kind: DomainKind::Memory {
                memory_bytes: Some(64 * 1024 * 1024 * 1024),
            },
            id: 9,
            processors: ProcessorSet::empty(),
        };
        let json = serde_json::to_string(&domain).expect("serialize");
        assert!(
            json.contains(r#""processors":[]"#),
            "unexpected JSON: {json}"
        );
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn a_memory_domain_with_unknown_size_omits_memory_bytes_rather_than_writing_null() {
        let domain = Domain {
            kind: DomainKind::Memory { memory_bytes: None },
            id: 0,
            processors: ProcessorSet::empty(),
        };
        let json = serde_json::to_string(&domain).expect("serialize");
        assert!(!json.contains("memory_bytes"), "unexpected JSON: {json}");
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn an_unrecognised_domain_kind_round_trips_its_attributes_losslessly() {
        let mut attributes = BTreeMap::new();
        attributes.insert("watts".to_string(), AttributeValue::Float(15.5));
        attributes.insert("throttled".to_string(), AttributeValue::Bool(false));
        let domain = Domain {
            kind: DomainKind::Other {
                name: "power".to_string(),
                attributes,
            },
            id: 2,
            processors: ProcessorSet::empty(),
        };
        assert_eq!(round_trip(&domain), domain);
    }

    #[test]
    fn an_integer_attribute_beyond_f64s_precision_round_trips_exactly() {
        // PR #20 review response: 9,007,199,254,740,993 (2^53 + 1) is the
        // smallest positive integer an f64 cannot represent exactly -- it
        // would decode as 9,007,199,254,740,992 if this ever routed through
        // `f64` again. Also covers a large negative integer, which only the
        // signed variant can hold.
        let large_unsigned: u64 = (1u64 << 53) + 1;
        let large_signed: i64 = -((1i64 << 53) + 1);
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "large_unsigned".to_string(),
            AttributeValue::UnsignedInteger(large_unsigned),
        );
        attributes.insert(
            "large_signed".to_string(),
            AttributeValue::SignedInteger(large_signed),
        );
        let domain = Domain {
            kind: DomainKind::Other {
                name: "precision".to_string(),
                attributes,
            },
            id: 3,
            processors: ProcessorSet::empty(),
        };
        let restored = round_trip(&domain);
        assert_eq!(restored, domain);
        let DomainKind::Other { attributes, .. } = &restored.kind else {
            panic!("expected Other")
        };
        assert_eq!(
            attributes.get("large_unsigned"),
            Some(&AttributeValue::UnsignedInteger(large_unsigned))
        );
        assert_eq!(
            attributes.get("large_signed"),
            Some(&AttributeValue::SignedInteger(large_signed))
        );
    }

    #[test]
    fn a_memory_bytes_value_beyond_f64s_precision_decodes_exactly() {
        // PR #20 review response: `memory_bytes` is decoded through the same
        // `as_u64` helper as any other unsigned attribute, so it must
        // preserve the same precision beyond 2^53.
        let precise: u64 = (1u64 << 53) + 1;
        let domain = Domain {
            kind: DomainKind::Memory {
                memory_bytes: Some(precise),
            },
            id: 4,
            processors: ProcessorSet::empty(),
        };
        let restored = round_trip(&domain);
        let DomainKind::Memory { memory_bytes } = restored.kind else {
            panic!("expected Memory")
        };
        assert_eq!(memory_bytes, Some(precise));
    }

    #[test]
    fn an_unrecognised_domain_kinds_attribute_colliding_with_a_reserved_field_name_is_refused() {
        for reserved in ["kind", "id", "processors"] {
            let mut attributes = BTreeMap::new();
            attributes.insert(reserved.to_string(), AttributeValue::UnsignedInteger(1));
            let domain = Domain {
                kind: DomainKind::Other {
                    name: "power".to_string(),
                    attributes,
                },
                id: 2,
                processors: ProcessorSet::empty(),
            };
            serde_json::to_string(&domain).expect_err(&format!(
                "an attribute named {reserved:?} must not silently overwrite the reserved field"
            ));
        }
    }

    #[test]
    fn a_hand_written_synthetic_description_parses() {
        // The whole point of an open, hand-writable schema: no discovery, no
        // ProcessorSet builder API, just JSON text a human could type.
        let json = r#"{
            "kind": "memory",
            "id": 5,
            "processors": [],
            "memory_bytes": 549755813888
        }"#;
        let domain: Domain = serde_json::from_str(json).expect("parse");
        assert_eq!(domain.id, 5);
        assert!(domain.processors.is_empty());
        assert_eq!(
            domain.kind,
            DomainKind::Memory {
                memory_bytes: Some(549_755_813_888)
            }
        );
    }

    #[test]
    fn a_hand_written_description_can_place_kind_in_any_field_order() {
        // Buffering to a map before interpreting `kind` (see the module doc
        // on `serde_impl`) is what makes this safe rather than fragile.
        let json = r#"{
            "processors": [{"group": 0, "number": 0}],
            "id": 1,
            "kind": "package"
        }"#;
        let domain: Domain = serde_json::from_str(json).expect("parse");
        assert_eq!(domain.kind, DomainKind::Package);
    }

    #[test]
    fn an_unrecognised_kind_from_hand_written_json_parses_as_other() {
        let json = r#"{
            "kind": "die",
            "id": 0,
            "processors": [{"group": 0, "number": 0}]
        }"#;
        // "die" IS recognised -- this proves the well-known path, contrasted
        // with a genuinely unknown kind below.
        let domain: Domain = serde_json::from_str(json).expect("parse");
        assert_eq!(domain.kind, DomainKind::Die);

        let json = r#"{
            "kind": "quantum-cache",
            "id": 0,
            "processors": [],
            "coherence": "eventual"
        }"#;
        let domain: Domain = serde_json::from_str(json).expect("parse");
        let DomainKind::Other { name, attributes } = domain.kind else {
            panic!("expected Other")
        };
        assert_eq!(name, "quantum-cache");
        assert_eq!(
            attributes.get("coherence"),
            Some(&AttributeValue::String("eventual".to_string()))
        );
    }

    #[test]
    fn a_missing_required_field_is_a_clean_error_not_a_panic() {
        let json = r#"{"kind": "group", "id": 0}"#;
        let error = serde_json::from_str::<Domain>(json).expect_err("processors is required");
        assert!(
            error.to_string().contains("processors"),
            "error should name the missing field: {error}"
        );
    }
}
