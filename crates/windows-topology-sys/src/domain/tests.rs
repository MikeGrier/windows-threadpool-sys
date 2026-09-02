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

    // --- float-to-integer coercion (mutation-testing gap) ---
    //
    // `as_u64`/`as_i64` accept a JSON float only when it is a whole number
    // inside the target's range. That guard was added as a PR #20 review
    // response, specifically to stop the silent precision loss the older
    // f64-for-everything encoding had -- and a `cargo mutants` run found that
    // *every* operator in it survives mutation. Replacing the whole guard with
    // `true` or `false`, flipping `==` to `!=`, or `&&` to `||`, all left the
    // suite green, because no test ever fed a float into an integer field.
    //
    // A correction made in response to review, never verified, is the worst
    // case of this: the reasoning is on record and the behaviour is not.
    //
    // These matter for a hand-written or fed-in description, which is the whole
    // reason the schema is open: JSON has one number type, so a generator that
    // emits 4.0 for a count is entirely ordinary, and one that emits 4.5 is a
    // defect that must be refused rather than truncated.

    /// A memory domain whose `memory_bytes` is written as the given JSON number
    /// literal, which is the shortest path to `as_u64`.
    fn memory_domain_with(bytes_literal: &str) -> Result<Domain, serde_json::Error> {
        let json = format!(
            r#"{{"kind": "memory", "id": 0, "processors": [], "memory_bytes": {bytes_literal}}}"#
        );
        serde_json::from_str(&json)
    }

    #[test]
    fn a_whole_number_float_is_accepted_as_an_unsigned_field() {
        let domain = memory_domain_with("1024.0").expect("4.0 is a whole number");
        assert_eq!(
            domain.kind,
            DomainKind::Memory {
                memory_bytes: Some(1024)
            },
            "a generator emitting a whole number as a float is ordinary JSON"
        );
    }

    #[test]
    fn a_fractional_float_is_refused_rather_than_truncated() {
        // The precision-loss case the guard exists for. Truncating to 1024
        // would be silent data corruption in a field describing a machine.
        assert!(
            memory_domain_with("1024.5").is_err(),
            "a fractional byte count must be refused, not rounded"
        );
    }

    #[test]
    fn a_negative_float_is_refused_for_an_unsigned_field() {
        assert!(
            memory_domain_with("-1.0").is_err(),
            "a negative count is out of range for an unsigned field"
        );
    }

    #[test]
    fn a_float_beyond_the_unsigned_range_is_refused() {
        // Above u64::MAX. `n as u64` would saturate silently, which is exactly
        // the conversion the range half of the guard prevents.
        assert!(
            memory_domain_with("1e300").is_err(),
            "a float larger than u64::MAX must be refused, not saturated"
        );
    }

    #[test]
    fn zero_is_accepted_at_the_bottom_of_the_unsigned_range() {
        // The boundary the range check includes. A guard written with an
        // exclusive bound would wrongly refuse this.
        let domain = memory_domain_with("0.0").expect("zero is in range");
        assert_eq!(
            domain.kind,
            DomainKind::Memory {
                memory_bytes: Some(0)
            }
        );
    }

    #[test]
    fn an_integer_literal_still_parses_unchanged() {
        // The common path, asserted beside the float ones so a guard that
        // refused everything could not pass this group.
        let domain = memory_domain_with("4096").expect("plain integers parse");
        assert_eq!(
            domain.kind,
            DomainKind::Memory {
                memory_bytes: Some(4096)
            }
        );
    }

    /// A cache domain whose `cache_type` carries a raw signed code, which is
    /// the path to `as_i64`.
    fn cache_domain_with_other_type(code_literal: &str) -> Result<Domain, serde_json::Error> {
        let json = format!(
            r#"{{"kind": "cache", "id": 0, "processors": [],
                 "level": 2, "associativity": 8, "line_size": 64,
                 "size_bytes": 1024, "cache_type": {{"other": {code_literal}}}}}"#
        );
        serde_json::from_str(&json)
    }

    #[test]
    fn a_whole_number_float_is_accepted_as_a_signed_field() {
        let domain = cache_domain_with_other_type("9.0").expect("9.0 is a whole number");
        let DomainKind::Cache { cache_type, .. } = domain.kind else {
            panic!("expected a cache domain");
        };
        assert_eq!(cache_type, CacheKind::Other(9));
    }

    #[test]
    fn a_negative_whole_float_is_accepted_as_a_signed_field() {
        // The signed range genuinely extends below zero -- a raw
        // PROCESSOR_CACHE_TYPE is an i32 and is not guaranteed non-negative --
        // so this is the case that distinguishes `as_i64`'s guard from
        // `as_u64`'s rather than duplicating it.
        let domain = cache_domain_with_other_type("-3.0").expect("-3.0 is a whole number");
        let DomainKind::Cache { cache_type, .. } = domain.kind else {
            panic!("expected a cache domain");
        };
        assert_eq!(cache_type, CacheKind::Other(-3));
    }

    #[test]
    fn a_fractional_float_is_refused_for_a_signed_field() {
        assert!(
            cache_domain_with_other_type("-3.5").is_err(),
            "a fractional cache-type code must be refused, not truncated"
        );
    }

    #[test]
    fn a_float_beyond_the_signed_range_is_refused() {
        assert!(
            cache_domain_with_other_type("-1e300").is_err(),
            "a float below i64::MIN must be refused, not saturated"
        );
    }
    // -----------------------------------------------------------------------
    // Rejection.
    //
    // Every test above round-trips a value this crate serialized, so the
    // deserializer only ever sees well-formed input and its refusal paths are
    // never taken. A mutation run replaced `as_bool` with a constant `true` and
    // loosened the cache-object guard, and neither could fail against input
    // that was already valid.
    //
    // These start from hand-written JSON instead.
    // -----------------------------------------------------------------------

    #[test]
    fn a_false_flag_survives_the_round_trip_as_false() {
        // `as_bool -> Ok(true)` survived because every serde test above used
        // `simultaneous_multithreading: true`. With only that value in the
        // suite, a deserializer that ignored its input and always answered
        // `true` was indistinguishable from one that read it.
        let domain = Domain {
            kind: DomainKind::Core {
                simultaneous_multithreading: false,
                efficiency_class: 0,
            },
            id: 1,
            processors: ProcessorSet::from_group_mask(0, 0b1),
        };

        let restored = round_trip(&domain);
        assert_eq!(restored, domain);
        let DomainKind::Core {
            simultaneous_multithreading,
            ..
        } = restored.kind
        else {
            panic!("a core domain must deserialize as one");
        };
        assert!(
            !simultaneous_multithreading,
            "a machine without SMT must not be reported as having it"
        );
    }

    #[test]
    fn a_non_boolean_smt_flag_is_refused_rather_than_read_as_true() {
        // The other half: a constant `true` accepts input that is not a boolean
        // at all. Anything that decodes to a different `AttributeValue` reaches
        // the refusal, so a string and a number are both tried.
        // The well-formed shape is asserted first, so a later failure is
        // attributable to the flag rather than to the rest of the description.
        // Written after exactly that mistake: an ad-hoc `processors` literal
        // made the refusals happen for an unrelated reason.
        let well_formed = r#"{"kind": "core", "id": 1, "processors": [],
                 "simultaneous_multithreading": false, "efficiency_class": 0}"#;
        serde_json::from_str::<Domain>(well_formed)
            .expect("the fixture must parse when only the flag is changed");

        for bad in [r#""yes""#, "1", "null"] {
            let json = well_formed.replace("false", bad);
            let error = serde_json::from_str::<Domain>(&json)
                .expect_err("{bad} is not a boolean and must not be read as one");
            assert!(
                error.to_string().contains("boolean"),
                "the refusal must say what was expected: {error}"
            );
        }
    }

    #[test]
    fn a_cache_type_object_carrying_more_than_the_other_key_is_refused() {
        // The `map.len() == 1` guard survived being replaced with `true`, which
        // would accept an object with extra keys and silently ignore them. That
        // matters more than it looks: the object form exists to carry a raw
        // `PROCESSOR_CACHE_TYPE` this crate does not recognise, so a reader
        // sending a *newer* shape must be told it was not understood rather
        // than have the parts we recognise quietly taken.
        //
        // Built from the same helper as the accepted case below, which is what
        // makes the refusal attributable: an ad-hoc JSON literal here was
        // rejected for a malformed `processors` field instead, and would have
        // passed this assertion while testing nothing about the guard.
        let json = r#"{"kind": "cache", "id": 0, "processors": [],
                 "level": 2, "associativity": 8, "line_size": 64,
                 "size_bytes": 1024, "cache_type": {"other": 99, "extra": 1}}"#;

        let outcome: Result<Domain, _> = serde_json::from_str(json);
        let error = outcome.expect_err("an object with a second key must be refused");
        assert!(
            error.to_string().contains("cache_type"),
            "the refusal must name the field that was not understood, or a \
             reader cannot tell which part of their description was rejected: {error}"
        );
    }

    #[test]
    fn a_cache_type_object_with_exactly_the_other_key_is_accepted() {
        // The positive case, so the test above cannot be satisfied by a guard
        // that refuses every object -- or, as it first was, by a fixture that
        // never reached the guard at all.
        let domain =
            cache_domain_with_other_type("99").expect("the single-key form is the one we emit");
        let DomainKind::Cache { cache_type, .. } = domain.kind else {
            panic!("a cache domain must deserialize as one");
        };
        assert_eq!(cache_type, CacheKind::Other(99));
    }
}
