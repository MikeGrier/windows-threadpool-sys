// Copyright (c) 2026 Mike Grier
//! The table of events (DE-D-2): that it is consistent with itself, and the facts that identify the provider and its events and
//! that a reader decodes by.

use durable_ioring::DiagnosticKind;

use super::{
    ColumnType, EVENTS, KEYWORDS, PROVIDER_ID, PROVIDER_NAME, VERSION, id, schema_of_id,
    schema_of_kind,
};

const KINDS: [DiagnosticKind; 5] = [
    DiagnosticKind::Inconsistency,
    DiagnosticKind::LockWait,
    DiagnosticKind::FlushIssued,
    DiagnosticKind::FlushAnswered,
    DiagnosticKind::SealWaited,
];

#[test]
fn the_provider_is_identified_by_the_name_and_guid_it_was_given() {
    // Pinned: changing either breaks every session that enabled the provider by it.
    assert_eq!(PROVIDER_NAME, "durable-ioring-events");
    assert_eq!(
        PROVIDER_ID.to_string(),
        "{47067f6f-8f2b-4278-9b92-092dfb3385e6}"
    );
}

#[test]
fn the_ids_are_the_published_numbers_in_ascending_order() {
    // Pinned: a reader decodes by them.
    let ids: Vec<u16> = EVENTS.iter().map(|event| event.id).collect();
    assert_eq!(
        ids,
        [
            id::INCONSISTENCY,
            id::LOCK_WAIT,
            id::FLUSH_ISSUED,
            id::FLUSH_ANSWERED_FILE,
            id::FLUSH_ANSWERED_DOMAIN,
            id::SEAL_WAITED
        ]
    );
    assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
}

#[test]
fn no_two_events_share_an_id_or_a_name() {
    for (i, a) in EVENTS.iter().enumerate() {
        for b in &EVENTS[i + 1..] {
            assert_ne!(a.id, b.id);
            assert_ne!(a.name, b.name);
        }
    }
}

#[test]
fn every_event_is_in_the_order_of_its_id() {
    assert!(EVENTS.windows(2).all(|pair| pair[0].id < pair[1].id));
}

#[test]
fn every_event_has_a_nonzero_level_and_keyword_as_the_page_asks() {
    for event in &EVENTS {
        assert_ne!(event.level, 0, "{}", event.name);
        assert_ne!(event.keyword.mask, 0, "{}", event.name);
    }
}

#[test]
fn every_keyword_is_one_bit_in_the_providers_forty_eight() {
    for keyword in KEYWORDS {
        assert_eq!(keyword.mask.count_ones(), 1, "{}", keyword.name);
        assert_eq!(keyword.mask & 0xffff_0000_0000_0000, 0, "{}", keyword.name);
    }
}

#[test]
fn no_two_keywords_share_a_bit_or_a_name() {
    for (i, a) in KEYWORDS.iter().enumerate() {
        for b in &KEYWORDS[i + 1..] {
            assert_eq!(a.mask & b.mask, 0);
            assert_ne!(a.name, b.name);
        }
    }
}

#[test]
fn every_events_keyword_is_one_of_the_listed_keywords() {
    for event in &EVENTS {
        assert!(
            KEYWORDS
                .iter()
                .any(|keyword| std::ptr::eq(*keyword, event.keyword)),
            "{}",
            event.name
        );
    }
}

#[test]
fn every_keyword_is_used_by_an_event() {
    for keyword in KEYWORDS {
        assert!(
            EVENTS
                .iter()
                .any(|event| std::ptr::eq(event.keyword, keyword)),
            "{}",
            keyword.name
        );
    }
}

#[test]
fn the_events_of_one_kind_share_a_level_and_a_keyword() {
    // The sink asks about a kind by its first event, so the others must agree with it.
    for kind in KINDS {
        let of_kind: Vec<_> = EVENTS.iter().filter(|event| event.kind == kind).collect();
        assert!(!of_kind.is_empty(), "{kind:?} has no event");
        for event in &of_kind {
            assert_eq!(event.level, of_kind[0].level, "{}", event.name);
            assert_eq!(
                event.keyword.mask, of_kind[0].keyword.mask,
                "{}",
                event.name
            );
        }
    }
}

#[test]
fn every_event_belongs_to_a_kind_the_sink_is_asked_about() {
    for event in &EVENTS {
        assert!(KINDS.contains(&event.kind), "{}", event.name);
    }
}

#[test]
fn a_kind_is_asked_about_by_its_first_event() {
    for kind in KINDS {
        let first = EVENTS
            .iter()
            .find(|event| event.kind == kind)
            .expect("an event");
        assert_eq!(schema_of_kind(kind).map(|event| event.id), Some(first.id));
    }
}

#[test]
fn an_event_is_found_by_its_id_and_an_unknown_id_is_none() {
    for event in &EVENTS {
        assert_eq!(
            schema_of_id(event.id).map(|found| found.name),
            Some(event.name)
        );
    }
    assert!(schema_of_id(0).is_none());
    assert!(schema_of_id(7).is_none());
    assert!(schema_of_id(u16::MAX).is_none());
}

#[test]
fn no_event_has_two_fields_of_one_name() {
    for event in &EVENTS {
        for (i, a) in event.columns.iter().enumerate() {
            for b in &event.columns[i + 1..] {
                assert_ne!(a.name, b.name, "{}", event.name);
            }
        }
    }
}

#[test]
fn every_binary_field_names_an_earlier_four_byte_length() {
    for event in &EVENTS {
        for (i, column) in event.columns.iter().enumerate() {
            if let ColumnType::Binary { length } = column.ty {
                let found = event.columns[..i]
                    .iter()
                    .find(|earlier| earlier.name == length)
                    .unwrap_or_else(|| {
                        panic!("{} names a length that is not earlier", column.name)
                    });
                assert_eq!(found.ty, ColumnType::UInt32, "{}", column.name);
            }
        }
    }
}

#[test]
fn every_event_starts_with_the_instance_so_a_reader_can_tell_instances_apart() {
    for event in &EVENTS {
        assert_eq!(event.columns[0].name, "Instance", "{}", event.name);
        assert_eq!(event.columns[0].ty, ColumnType::UInt64);
    }
}

#[test]
fn the_descriptor_is_the_events_id_level_and_keyword_with_the_defaults_the_page_names() {
    for event in &EVENTS {
        let descriptor = event.descriptor();
        assert_eq!(descriptor.id, event.id);
        assert_eq!(descriptor.level, event.level);
        assert_eq!(descriptor.keyword, event.keyword.mask);
        assert_eq!(descriptor.version, VERSION);
        assert_eq!(
            (descriptor.channel, descriptor.opcode, descriptor.task),
            (0, 0, 0)
        );
    }
}

#[test]
fn an_inconsistency_is_an_error_and_every_delay_is_informational() {
    for event in &EVENTS {
        let expected = if event.kind == DiagnosticKind::Inconsistency {
            win_etw_sys::level::ERROR
        } else {
            win_etw_sys::level::INFO
        };
        assert_eq!(event.level, expected, "{}", event.name);
    }
}
