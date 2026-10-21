// Copyright (c) 2026 Mike Grier
//! What an event of durable-ioring is written as (DE-D-2): the event it becomes, the values of its fields in the order the table
//! gives, and that those agree with the table in type and in the length a `Binary` field is told.

use std::time::Duration;

use durable_ioring::{Diagnostic, FlushDomain, FlushPart, IoTargetKey};

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

#[test]
fn a_lock_wait_is_its_instance_lock_and_nanoseconds() {
    let event = Diagnostic::LockWait {
        instance: 4,
        lock: "engine",
        waited: Duration::from_nanos(12),
    };
    agrees(&event);
    assert_eq!(schema_of(&event).map(|s| s.id), Some(id::LOCK_WAIT));
    assert_eq!(
        encode(&event),
        Some(vec![Value::UInt64(4), text("engine"), Value::UInt64(12)])
    );
}

#[test]
fn a_flush_issued_is_its_identity_the_epoch_as_text_and_the_counts() {
    let event = Diagnostic::FlushIssued {
        instance: 2,
        lineage: 1,
        through: format_args!("{:?}", 5_u64),
        files: 3,
        domains: 1,
    };
    agrees(&event);
    assert_eq!(schema_of(&event).map(|s| s.id), Some(id::FLUSH_ISSUED));
    assert_eq!(
        encode(&event),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("5"),
            Value::UInt64(3),
            Value::UInt64(1)
        ])
    );
}

#[test]
fn a_file_flush_answered_is_the_file_key_the_time_and_whether_it_succeeded() {
    let event = Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::File(IoTargetKey(77)),
        elapsed: Duration::from_nanos(15),
        succeeded: false,
    };
    agrees(&event);
    assert_eq!(
        schema_of(&event).map(|s| s.id),
        Some(id::FLUSH_ANSWERED_FILE)
    );
    assert_eq!(
        encode(&event),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("9"),
            Value::UInt64(77),
            Value::UInt64(15),
            Value::Boolean(false)
        ])
    );
}

#[test]
fn a_domain_flush_answered_is_the_domains_bytes_with_their_length_first() {
    let domain = FlushDomain::new(b"ssd-4".to_vec());
    let event = Diagnostic::FlushAnswered {
        instance: 2,
        lineage: 1,
        through: format_args!("9"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::from_nanos(40),
        succeeded: true,
    };
    agrees(&event);
    assert_eq!(
        schema_of(&event).map(|s| s.id),
        Some(id::FLUSH_ANSWERED_DOMAIN)
    );
    assert_eq!(
        encode(&event),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(1),
            text("9"),
            Value::UInt32(5),
            Value::Bytes(b"ssd-4".to_vec()),
            Value::UInt64(40),
            Value::Boolean(true)
        ])
    );
}

#[test]
fn a_domain_whose_name_is_not_text_is_written_as_the_bytes_it_is() {
    let domain = FlushDomain::new(vec![0xff, 0, 0x61]);
    let event = Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::ZERO,
        succeeded: true,
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(values[3], Value::UInt32(3));
    assert_eq!(values[4], Value::Bytes(vec![0xff, 0, 0x61]));
}

#[test]
fn an_empty_domain_name_is_a_length_of_zero_and_no_bytes() {
    let domain = FlushDomain::new(Vec::new());
    let event = Diagnostic::FlushAnswered {
        instance: 1,
        lineage: 0,
        through: format_args!("1"),
        part: FlushPart::Domain(&domain),
        elapsed: Duration::ZERO,
        succeeded: true,
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(values[3], Value::UInt32(0));
    assert_eq!(values[4], Value::Bytes(Vec::new()));
}

#[test]
fn a_seal_waited_is_its_identity_the_epoch_as_text_and_the_time() {
    let event = Diagnostic::SealWaited {
        instance: 2,
        lineage: 0,
        through: format_args!("1"),
        waited: Duration::from_nanos(30),
    };
    agrees(&event);
    assert_eq!(schema_of(&event).map(|s| s.id), Some(id::SEAL_WAITED));
    assert_eq!(
        encode(&event),
        Some(vec![
            Value::UInt64(2),
            Value::UInt64(0),
            text("1"),
            Value::UInt64(30)
        ])
    );
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
fn a_duration_too_long_for_a_u64_of_nanoseconds_is_the_largest() {
    let event = Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::MAX,
    };
    assert_eq!(encode(&event).expect("values")[2], Value::UInt64(u64::MAX));
}

#[test]
fn a_zero_duration_is_zero_nanoseconds() {
    let event = Diagnostic::LockWait {
        instance: 1,
        lock: "engine",
        waited: Duration::ZERO,
    };
    assert_eq!(encode(&event).expect("values")[2], Value::UInt64(0));
}

#[test]
fn a_counts_largest_value_is_kept() {
    let event = Diagnostic::FlushIssued {
        instance: u64::MAX,
        lineage: u64::MAX,
        through: format_args!(""),
        files: usize::MAX,
        domains: 0,
    };
    agrees(&event);
    let values = encode(&event).expect("values");
    assert_eq!(values[0], Value::UInt64(u64::MAX));
    assert_eq!(values[3], Value::UInt64(usize::MAX as u64));
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
