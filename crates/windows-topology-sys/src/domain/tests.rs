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
    attributes.insert("watts".to_string(), AttributeValue::Number(15.5));
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
