// Copyright (c) 2026 Mike Grier
//! What an event of durable-ioring is written as (DE-D-2): the event it becomes, the values of its fields in the order the table
//! gives, and that those agree with the table in type and in the length a `Binary` field is told.

use durable_ioring::{
    CorrelationId, Diagnostic, DiagnosticKind, DiagnosticSink, FlushDomain, FlushOutcome,
    FlushPart, IoTargetKey, SealOutcome,
};

use super::{Value, encode, schema_of};
use crate::schema::{ColumnType, id};

/// That `values` are the schema's fields: as many, each of the column's type, and a `Binary` as long as its length column says.
fn agrees(event: &Diagnostic<'_>) {
    let schema = schema_of(event).expect("an event this crate knows");
    let values = encode(event).expect("values");
    assert_eq!(values.len(), schema.columns.len(), "{}", schema.name);
    for (value, column) in values.iter().zip(schema.columns) {
        assert!(
            value.is_of(column.ty),
            "{} {}: {value:?}",
            schema.name,
            column.name
        );
        if let (Value::Bytes(bytes), ColumnType::Binary { length }) = (value, column.ty) {
            let at = schema
                .columns
                .iter()
                .position(|c| c.name == length)
                .expect("a length");
            assert_eq!(
                values[at],
                Value::UInt32(u32::try_from(bytes.len()).expect("small"))
            );
        }
    }
}

fn text(s: &str) -> Value {
    Value::Text(s.encode_utf16().collect())
}

#[test]
fn an_inconsistency_is_its_instance_what_and_detail() {
    let event = Diagnostic::Inconsistency {
        instance: 7,
        what: "a write completed twice",
        detail: format_args!("lineage {}", 3),
    };
    agrees(&event);
    assert_eq!(schema_of(&event).map(|s| s.id), Some(id::INCONSISTENCY));
    assert_eq!(
        encode(&event),
        Some(vec![
            Value::UInt64(7),
            text("a write completed twice"),
            text("lineage 3")
        ])
    );
}

/// A sink that wants every kind, to mint ids from.
struct Wanting;

impl DiagnosticSink for Wanting {
    fn wants(&self, _: DiagnosticKind) -> bool {
        true
    }

    fn report(&self, _: &Diagnostic<'_>) {}
}

fn minted() -> CorrelationId {
    CorrelationId::for_pair(&Wanting, DiagnosticKind::Seal).expect("wanted")
}

/// The schema says which end of a pair an event is exactly as durable-ioring does, since the opcode follows from it.
fn edge_agrees(event: &Diagnostic<'_>) {
    let schema = schema_of(event).expect("an event this crate knows");
    assert_eq!(
        schema.edge,
        event.pairing().map(|pairing| pairing.edge),
        "{}",
        schema.name
    );
    assert_eq!(Some(schema.kind), Some(event.kind()), "{}", schema.name);
}

#[test]
fn a_seals_begin_is_its_identity_the_epoch_as_text_and_its_pair() {
    let pair = minted();
    let event = Diagnostic::SealBegan {
        instance: 2,
        lineage: 1,
        through: format_args!("{:?}", 5_u64),
        id: pair,
    };
    agrees(&event);
    edge_agrees(&event);
    assert_eq!(schema_of(&event).map(|s| s.id), Some(id::SEAL_BEGAN));
    assert_eq!(
        encode(&event),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("5"),
            Value::UInt64(pair.get())
        ])
    );
}

#[test]
fn a_seals_end_is_its_identity_its_pair_and_how_it_ended() {
    let pair = minted();
    for (outcome, said) in [
        (SealOutcome::Durable, "durable"),
        (SealOutcome::Failed, "failed"),
        (SealOutcome::Blocked, "blocked"),
        (SealOutcome::Ended, "ended unanswered"),
    ] {
        let event = Diagnostic::SealEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("5"),
            id: pair,
            outcome,
        };
        agrees(&event);
        edge_agrees(&event);
        assert_eq!(schema_of(&event).map(|s| s.id), Some(id::SEAL_ENDED));
        assert_eq!(
            encode(&event),
            Some(vec![
                Value::UInt64(2),
                Value::UInt64(1),
                text("5"),
                Value::UInt64(pair.get()),
                text(said)
            ])
        );
    }
}

#[test]
fn a_files_flush_is_a_pair_naming_the_file_key() {
    let pair = minted();
    let began = Diagnostic::FlushBegan {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(77)),
        id: pair,
    };
    agrees(&began);
    edge_agrees(&began);
    assert_eq!(schema_of(&began).map(|s| s.id), Some(id::FLUSH_BEGAN_FILE));
    assert_eq!(
        encode(&began),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("9"),
            Value::UInt64(77),
            Value::UInt64(pair.get())
        ])
    );
    let ended = Diagnostic::FlushEnded {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(77)),
        id: pair,
        outcome: FlushOutcome::Failed,
    };
    agrees(&ended);
    edge_agrees(&ended);
    assert_eq!(schema_of(&ended).map(|s| s.id), Some(id::FLUSH_ENDED_FILE));
    assert_eq!(
        encode(&ended),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("9"),
            Value::UInt64(77),
            Value::UInt64(pair.get()),
            text("failed")
        ])
    );
}

#[test]
fn a_domains_flush_is_a_pair_naming_the_domains_bytes_with_their_length_first() {
    let pair = minted();
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    let began = Diagnostic::FlushBegan {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        id: pair,
    };
    agrees(&began);
    edge_agrees(&began);
    assert_eq!(
        schema_of(&began).map(|s| s.id),
        Some(id::FLUSH_BEGAN_DOMAIN)
    );
    assert_eq!(
        encode(&began),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("9"),
            Value::UInt32(5),
            Value::Bytes(b"ssd-4".to_vec()),
            Value::UInt64(pair.get())
        ])
    );
    for (outcome, said) in [
        (FlushOutcome::Succeeded, "succeeded"),
        (FlushOutcome::Abandoned, "abandoned"),
    ] {
        let ended = Diagnostic::FlushEnded {
            instance: 2,
            lineage: 1,
            through: format_args!("9"),
            part: FlushPart::Domain(&domain),
            id: pair,
            outcome,
        };
        agrees(&ended);
        edge_agrees(&ended);
        assert_eq!(
            schema_of(&ended).map(|s| s.id),
            Some(id::FLUSH_ENDED_DOMAIN)
        );
        assert_eq!(encode(&ended).expect("values")[6], text(said));
    }
}

#[test]
fn a_domain_whose_name_is_not_text_is_written_as_the_bytes_it_is() {
    let domain = FlushDomain::new(vec![0xff, 0, 0x61]);
    let event = Diagnostic::FlushBegan {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        id: minted(),
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(values[3], Value::UInt32(3));
    assert_eq!(values[4], Value::Bytes(vec![0xff, 0, 0x61]));
}

#[test]
fn an_empty_domain_name_is_a_length_of_zero_and_no_bytes() {
    let domain = FlushDomain::new(Vec::new());
    let event = Diagnostic::FlushEnded {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        id: minted(),
        outcome: FlushOutcome::Succeeded,
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(values[3], Value::UInt32(0));
    assert_eq!(values[4], Value::Bytes(Vec::new()));
}

#[test]
fn a_lock_waits_pair_is_its_instance_its_lock_and_its_pair() {
    let pair = minted();
    let began = Diagnostic::LockWaitBegan {
        instance: 4,
        lock: "engine",
        id: pair,
    };
    let ended = Diagnostic::LockWaitEnded {
        instance: 4,
        lock: "engine",
        id: pair,
    };
    for (event, expected) in [(&began, id::LOCK_WAIT_BEGAN), (&ended, id::LOCK_WAIT_ENDED)] {
        agrees(event);
        edge_agrees(event);
        assert_eq!(schema_of(event).map(|s| s.id), Some(expected));
        assert_eq!(
            encode(event),
            Some(vec![
                Value::UInt64(4),
                text("engine"),
                Value::UInt64(pair.get())
            ])
        );
    }
}

#[test]
fn an_inconsistency_is_no_end_of_a_pair() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "x",
        detail: format_args!("y"),
    };
    edge_agrees(&event);
}

#[test]
fn text_outside_the_basic_plane_is_its_surrogate_pairs() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "\u{1f600}",
        detail: format_args!("\u{3042}"),
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(values[1], Value::Text(vec![0xd83d, 0xde00]));
    assert_eq!(values[2], Value::Text(vec![0x3042]));
}

#[test]
fn a_largest_identity_and_pair_are_kept() {
    let event = Diagnostic::SealBegan {
        instance: u64::MAX,
        lineage: u64::MAX,
        through: format_args!(""),
        id: minted(),
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(
        (values[0].clone(), values[1].clone()),
        (Value::UInt64(u64::MAX), Value::UInt64(u64::MAX))
    );
}

#[test]
fn a_value_is_of_its_column_type_and_no_other() {
    let cases = [
        (Value::UInt32(0), ColumnType::UInt32),
        (Value::UInt64(0), ColumnType::UInt64),
        (Value::Boolean(true), ColumnType::Boolean),
        (Value::Text(vec![]), ColumnType::UnicodeString),
        (Value::Bytes(vec![]), ColumnType::Binary { length: "L" }),
    ];
    for (i, (value, _)) in cases.iter().enumerate() {
        for (j, (_, ty)) in cases.iter().enumerate() {
            assert_eq!(value.is_of(*ty), i == j, "{value:?} against {ty:?}");
        }
    }
}

#[test]
fn a_boolean_is_written_as_four_bytes_and_text_with_a_terminator() {
    use win_etw_sys::Field;
    assert!(matches!(Value::Boolean(true).field(), Field::U32(1)));
    assert!(matches!(Value::Boolean(false).field(), Field::U32(0)));
    assert!(matches!(Value::UInt32(5).field(), Field::U32(5)));
    assert!(matches!(Value::UInt64(5).field(), Field::U64(5)));
    assert!(matches!(
        Value::Text(vec![1]).field(),
        Field::Utf16Terminated(_)
    ));
    assert!(matches!(Value::Bytes(vec![1]).field(), Field::Bytes(_)));
}

// --- a zero in text (DE-D-7) ---

/// Every text value an event's encoding holds.
fn texts(event: &Diagnostic<'_>) -> Vec<Vec<u16>> {
    encode(event)
        .expect("values")
        .into_iter()
        .filter_map(|value| match value {
            Value::Text(units) => Some(units),
            _ => None,
        })
        .collect()
}

#[test]
fn a_zero_in_text_ends_it_where_it_is() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "a\0b",
        detail: format_args!("c\0d\0e"),
    };
    agrees(&event);
    assert_eq!(
        encode(&event),
        Some(vec![Value::UInt64(1), text("a"), text("c")]),
        "what follows a zero is not part of the field"
    );
}

#[test]
fn text_that_starts_with_a_zero_is_empty() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "\0abc",
        detail: format_args!("\0"),
    };
    assert_eq!(texts(&event), [Vec::<u16>::new(), Vec::new()]);
}

#[test]
fn text_with_no_zero_is_kept_whole() {
    let event = Diagnostic::Inconsistency {
        instance: 1,
        what: "abc",
        detail: format_args!("{}", "d".repeat(1000)),
    };
    assert_eq!(texts(&event)[0], "abc".encode_utf16().collect::<Vec<_>>());
    assert_eq!(texts(&event)[1].len(), 1000);
}

#[test]
fn a_zero_in_any_text_of_any_event_leaves_every_other_field_where_it_was() {
    // The corruption a zero caused was in the fields after it, so the check is that the values after a cut text are the ones
    // that would be there without the zero, for each event with a text that is not the last field.
    let id = minted();
    let events = [
        Diagnostic::Inconsistency {
            instance: 5,
            what: "w\0x",
            detail: format_args!("d"),
        },
        Diagnostic::SealBegan {
            instance: 5,
            lineage: 6,
            through: format_args!("e\0x"),
            id,
        },
        Diagnostic::SealEnded {
            instance: 5,
            lineage: 6,
            through: format_args!("e\0x"),
            id,
            outcome: SealOutcome::Durable,
        },
        Diagnostic::FlushBegan {
            instance: 5,
            lineage: 6,
            through: format_args!("e\0x"),
            part: FlushPart::File(IoTargetKey(9)),
            id,
        },
        Diagnostic::FlushEnded {
            instance: 5,
            lineage: 6,
            through: format_args!("e\0x"),
            part: FlushPart::File(IoTargetKey(9)),
            id,
            outcome: FlushOutcome::Succeeded,
        },
        Diagnostic::LockWaitBegan {
            instance: 5,
            lock: "l\0x",
            id,
        },
        Diagnostic::LockWaitEnded {
            instance: 5,
            lock: "l\0x",
            id,
        },
    ];
    for event in &events {
        agrees(event);
        let values = encode(event).expect("values");
        assert!(
            texts(event).iter().all(|units| !units.contains(&0)),
            "{event}: {values:?}"
        );
        assert_eq!(values[0], Value::UInt64(5), "{event}");
        let pair = id.get();
        if event.pairing().is_some() {
            assert!(
                values.contains(&Value::UInt64(pair)),
                "{event}: the pair is where it was"
            );
        }
    }
}

#[test]
fn the_write_of_a_text_value_stops_at_a_zero_whoever_made_it() {
    use win_etw_sys::Field;
    let written = |units: Vec<u16>| -> Vec<u16> {
        let value = Value::Text(units);
        let Field::Utf16Terminated(written) = value.field() else {
            panic!("text is written terminated");
        };
        written.to_vec()
    };
    assert_eq!(written(vec![0x61, 0, 0x62]), [0x61]);
    assert!(written(vec![0, 0x62]).is_empty());
    assert_eq!(
        written(vec![0x61, 0x62]),
        [0x61, 0x62],
        "no zero, nothing cut"
    );
}
