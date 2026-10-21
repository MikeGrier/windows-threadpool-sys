// Copyright (c) 2026 Mike Grier
//! What is written to an event, from an event of durable-ioring, and the write of it (DE-D-2).

use std::time::Duration;

use durable_ioring::{Diagnostic, FlushPart};
use win_etw_sys::Field;

use crate::schema::{ColumnType, EVENTS, EventSchema, id};

#[cfg(test)]
mod tests;

/// A value of one field, owned: the text is encoded and the domain's bytes are copied, once the event is known to be wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// A `UInt32`.
    UInt32(u32),
    /// A `UInt64`.
    UInt64(u64),
    /// A `Boolean`.
    Boolean(bool),
    /// A `UnicodeString`, as UTF-16 units without the terminator.
    Text(Vec<u16>),
    /// A `Binary`.
    Bytes(Vec<u8>),
}

impl Value {
    /// Whether the value is of the column type: a `Binary` takes bytes whatever its length column says, which is held by a
    /// test and by [`encode`].
    #[must_use]
    pub fn is_of(&self, ty: ColumnType) -> bool {
        matches!(
            (self, ty),
            (Self::UInt32(_), ColumnType::UInt32)
                | (Self::UInt64(_), ColumnType::UInt64)
                | (Self::Boolean(_), ColumnType::Boolean)
                | (Self::Text(_), ColumnType::UnicodeString)
                | (Self::Bytes(_), ColumnType::Binary { .. })
        )
    }

    /// The field that writes this value.
    pub(crate) fn field(&self) -> Field<'_> {
        match self {
            Self::UInt32(v) => Field::U32(*v),
            Self::UInt64(v) => Field::U64(*v),
            Self::Boolean(v) => Field::boolean(*v),
            Self::Text(units) => Field::Utf16Terminated(units),
            Self::Bytes(bytes) => Field::Bytes(bytes),
        }
    }
}

/// The event an event of durable-ioring becomes, or `None` for one this crate does not know, which a newer durable-ioring may
/// add.
#[must_use]
pub fn schema_of(event: &Diagnostic<'_>) -> Option<&'static EventSchema> {
    let wanted = match event {
        Diagnostic::Inconsistency { .. } => id::INCONSISTENCY,
        Diagnostic::LockWait { .. } => id::LOCK_WAIT,
        Diagnostic::FlushIssued { .. } => id::FLUSH_ISSUED,
        Diagnostic::FlushAnswered {
            part: FlushPart::File(_),
            ..
        } => id::FLUSH_ANSWERED_FILE,
        Diagnostic::FlushAnswered {
            part: FlushPart::Domain(_),
            ..
        } => id::FLUSH_ANSWERED_DOMAIN,
        Diagnostic::SealWaited { .. } => id::SEAL_WAITED,
        _ => return None,
    };
    EVENTS.iter().find(|schema| schema.id == wanted)
}

/// The values of an event's fields, in the order of the schema's columns, or `None` for an event this crate does not know.
///
/// Text that is formatted, an epoch and an inconsistency's detail, is formatted here, which is why this is called only for an
/// event a session wants.
#[must_use]
pub fn encode(event: &Diagnostic<'_>) -> Option<Vec<Value>> {
    let units = |text: &str| Value::Text(text.encode_utf16().collect());
    let nanos =
        |duration: Duration| Value::UInt64(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
    let count = |n: usize| Value::UInt64(n as u64);
    Some(match event {
        Diagnostic::Inconsistency {
            instance,
            what,
            detail,
        } => vec![
            Value::UInt64(*instance),
            units(what),
            units(&detail.to_string()),
        ],
        Diagnostic::LockWait {
            instance,
            lock,
            waited,
        } => vec![Value::UInt64(*instance), units(lock), nanos(*waited)],
        Diagnostic::FlushIssued {
            instance,
            lineage,
            through,
            files,
            domains,
        } => vec![
            Value::UInt64(*instance),
            Value::UInt64(*lineage),
            units(&through.to_string()),
            count(*files),
            count(*domains),
        ],
        Diagnostic::FlushAnswered {
            instance,
            lineage,
            through,
            part,
            elapsed,
            succeeded,
        } => {
            let mut values = vec![
                Value::UInt64(*instance),
                Value::UInt64(*lineage),
                units(&through.to_string()),
            ];
            match part {
                FlushPart::File(key) => values.push(Value::UInt64(key.0)),
                FlushPart::Domain(domain) => {
                    let bytes = domain.bytes();
                    // A length that does not fit the column cuts the bytes to what it can say, so the two agree.
                    let kept = &bytes[..bytes.len().min(u32::MAX as usize)];
                    values.push(Value::UInt32(kept.len() as u32));
                    values.push(Value::Bytes(kept.to_vec()));
                }
                _ => return None,
            }
            values.push(nanos(*elapsed));
            values.push(Value::Boolean(*succeeded));
            values
        }
        Diagnostic::SealWaited {
            instance,
            lineage,
            through,
            waited,
        } => vec![
            Value::UInt64(*instance),
            Value::UInt64(*lineage),
            units(&through.to_string()),
            nanos(*waited),
        ],
        _ => return None,
    })
}
