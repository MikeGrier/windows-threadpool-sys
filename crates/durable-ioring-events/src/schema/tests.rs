// Copyright (c) 2026 Mike Grier
//! The table of events (DE-D-2): that it is consistent with itself, and the facts that identify the provider and its events and
//! that a reader decodes by.

use durable_ioring::{DiagnosticKind, Edge};

use super::{
    ColumnType, EVENTS, KEYWORDS, PROVIDER_ID, PROVIDER_NAME, TASKS, VERSION, id, opcode,
    schema_of_id, schema_of_kind,
};

const KINDS: [DiagnosticKind; 4] = [
    DiagnosticKind::Inconsistency,
    DiagnosticKind::Seal,
    DiagnosticKind::Flush,
    DiagnosticKind::LockWait,
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
            id::SEAL_BEGAN,
            id::SEAL_ENDED,
            id::FLUSH_BEGAN_FILE,
            id::FLUSH_ENDED_FILE,
            id::FLUSH_BEGAN_DOMAIN,
            id::FLUSH_ENDED_DOMAIN,
            id::LOCK_WAIT_BEGAN,
            id::LOCK_WAIT_ENDED
        ]
    );
    assert_eq!(ids, [1, 7, 8, 9, 10, 11, 12, 13, 14]);
}

#[test]
fn the_ids_of_the_retired_delay_events_are_not_used_again() {
    for retired in 2..=6 {
        assert!(schema_of_id(retired).is_none(), "id {retired} is in use");
    }
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
fn the_events_of_one_kind_share_a_level_a_keyword_and_a_task() {
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
            assert_eq!(event.task, of_kind[0].task, "{}", event.name);
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
    assert!(schema_of_id(15).is_none());
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
fn the_descriptor_is_the_events_id_level_keyword_task_and_opcode() {
    for event in &EVENTS {
        let descriptor = event.descriptor();
        assert_eq!(descriptor.id, event.id);
        assert_eq!(descriptor.level, event.level);
        assert_eq!(descriptor.keyword, event.keyword.mask);
        assert_eq!(descriptor.version, VERSION);
        assert_eq!(descriptor.channel, 0);
        assert_eq!(descriptor.task, event.task.map_or(0, |task| task.value));
        assert_eq!(descriptor.opcode, super::opcode(event.edge));
    }
}

#[test]
fn a_begin_is_written_as_start_an_end_as_stop_and_anything_else_as_info() {
    assert_eq!(super::opcode(None), opcode::INFO);
    assert_eq!(super::opcode(Some(Edge::Begin)), opcode::START);
    assert_eq!(super::opcode(Some(Edge::End)), opcode::STOP);
    assert_eq!(
        (opcode::INFO, opcode::START, opcode::STOP),
        (0, 1, 2),
        "as winmeta.xml defines them"
    );
}

#[test]
fn every_pair_kind_has_a_begin_and_an_end_with_a_task_and_nothing_else_has_either() {
    for kind in KINDS {
        let of_kind: Vec<_> = EVENTS.iter().filter(|event| event.kind == kind).collect();
        if kind == DiagnosticKind::Inconsistency {
            assert!(
                of_kind
                    .iter()
                    .all(|event| event.edge.is_none() && event.task.is_none())
            );
            continue;
        }
        assert!(
            of_kind.iter().any(|event| event.edge == Some(Edge::Begin)),
            "{kind:?}"
        );
        assert!(
            of_kind.iter().any(|event| event.edge == Some(Edge::End)),
            "{kind:?}"
        );
        assert!(of_kind.iter().all(|event| event.task.is_some()), "{kind:?}");
    }
}

#[test]
fn a_kind_is_asked_about_by_its_begin() {
    for kind in [
        DiagnosticKind::Seal,
        DiagnosticKind::Flush,
        DiagnosticKind::LockWait,
    ] {
        assert_eq!(
            schema_of_kind(kind).and_then(|event| event.edge),
            Some(Edge::Begin)
        );
    }
}

#[test]
fn every_tasks_value_is_its_own_and_not_the_default() {
    for (i, a) in TASKS.iter().enumerate() {
        assert_ne!(a.value, 0, "{}", a.name);
        for b in &TASKS[i + 1..] {
            assert_ne!(a.value, b.value);
            assert_ne!(a.name, b.name);
        }
    }
}

#[test]
fn every_events_task_is_one_of_the_listed_tasks_and_every_task_is_used() {
    for event in &EVENTS {
        if let Some(task) = event.task {
            assert!(
                TASKS.iter().any(|listed| std::ptr::eq(*listed, task)),
                "{}",
                event.name
            );
        }
    }
    for task in TASKS {
        assert!(
            EVENTS
                .iter()
                .any(|event| event.task.is_some_and(|t| std::ptr::eq(t, task))),
            "{}",
            task.name
        );
    }
}

#[test]
fn every_end_of_a_pair_carries_the_pair_and_every_end_says_how_it_ended_but_a_lock_waits() {
    for event in &EVENTS {
        let names: Vec<&str> = event.columns.iter().map(|c| c.name).collect();
        match event.edge {
            None => assert!(!names.contains(&"Pair"), "{}", event.name),
            Some(edge) => {
                assert!(names.contains(&"Pair"), "{}", event.name);
                let says = names.contains(&"Outcome");
                let should = edge == Edge::End && event.kind != DiagnosticKind::LockWait;
                assert_eq!(says, should, "{}", event.name);
            }
        }
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
