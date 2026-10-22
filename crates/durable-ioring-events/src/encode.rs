// Copyright (c) 2026 Mike Grier
//! What is written to an event, from an event of durable-ioring, and the write of it (DE-D-2).

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
    /// A `UnicodeString`, as UTF-16 units without the terminator, and **without a zero unit**: a zero in the text ends it where
    /// it is, as a reader of a `UnicodeString` ends it, so the units after it are not part of the value (DE-D-7).
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
            Self::Text(units) => Field::Utf16Terminated(before_zero(units)),
            Self::Bytes(bytes) => Field::Bytes(bytes),
        }
    }
}

/// `units` up to its first zero unit, which is where a reader of a `UnicodeString` field ends the string. A zero left in would end
/// it there for the reader while the units after it were taken as the next fields, so each later field of the event would be
/// misread (DE-D-7). The one place the rule is written: [`encode`] makes its text through it, and so does the write of a value
/// whoever made it.
fn before_zero(units: &[u16]) -> &[u16] {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    &units[..end]
}

/// The event an event of durable-ioring becomes, or `None` for one this crate does not know, which a newer durable-ioring may
/// add.
#[must_use]
pub fn schema_of(event: &Diagnostic<'_>) -> Option<&'static EventSchema> {
    let wanted = match event {
        Diagnostic::Inconsistency { .. } => id::INCONSISTENCY,
        Diagnostic::SealBegan { .. } => id::SEAL_BEGAN,
        Diagnostic::SealEnded { .. } => id::SEAL_ENDED,
        Diagnostic::FlushBegan { part, .. } => match part {
            FlushPart::File(_) => id::FLUSH_BEGAN_FILE,
            FlushPart::Domain(_) => id::FLUSH_BEGAN_DOMAIN,
            _ => return None,
        },
        Diagnostic::FlushEnded { part, .. } => match part {
            FlushPart::File(_) => id::FLUSH_ENDED_FILE,
            FlushPart::Domain(_) => id::FLUSH_ENDED_DOMAIN,
            _ => return None,
        },
        Diagnostic::LockWaitBegan { .. } => id::LOCK_WAIT_BEGAN,
        Diagnostic::LockWaitEnded { .. } => id::LOCK_WAIT_ENDED,
        _ => return None,
    };
    EVENTS.iter().find(|schema| schema.id == wanted)
}

/// The values of a flush part's columns: a file's key, or a domain's length and bytes.
fn part_values(part: &FlushPart<'_>) -> Option<Vec<Value>> {
    Some(match part {
        FlushPart::File(key) => vec![Value::UInt64(key.0)],
        FlushPart::Domain(domain) => {
            let bytes = domain.bytes();
            // A length that does not fit the column cuts the bytes to what it can say, so the two agree.
            let kept = &bytes[..bytes.len().min(u32::MAX as usize)];
            vec![
                Value::UInt32(kept.len() as u32),
                Value::Bytes(kept.to_vec()),
            ]
        }
        _ => return None,
    })
}

/// The values of an event's fields, in the order of the schema's columns, or `None` for an event this crate does not know.
///
/// Text that is formatted, an epoch and an inconsistency's detail, is formatted here, which is why this is called only for an
/// event a session wants.
#[must_use]
pub fn encode(event: &Diagnostic<'_>) -> Option<Vec<Value>> {
    let units = |text: &str| {
        let all: Vec<u16> = text.encode_utf16().collect();
        Value::Text(before_zero(&all).to_vec())
    };
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
        Diagnostic::SealBegan {
            instance,
            lineage,
            through,
            id,
        } => vec![
            Value::UInt64(*instance),
            Value::UInt64(*lineage),
            units(&through.to_string()),
            Value::UInt64(id.get()),
        ],
        Diagnostic::SealEnded {
            instance,
            lineage,
            through,
            id,
            outcome,
        } => vec![
            Value::UInt64(*instance),
            Value::UInt64(*lineage),
            units(&through.to_string()),
            Value::UInt64(id.get()),
            units(&outcome.to_string()),
        ],
        Diagnostic::FlushBegan {
            instance,
            lineage,
            through,
            part,
            id,
        } => {
            let mut values = vec![
                Value::UInt64(*instance),
                Value::UInt64(*lineage),
                units(&through.to_string()),
            ];
            values.extend(part_values(part)?);
            values.push(Value::UInt64(id.get()));
            values
        }
        Diagnostic::FlushEnded {
            instance,
            lineage,
            through,
            part,
            id,
            outcome,
        } => {
            let mut values = vec![
                Value::UInt64(*instance),
                Value::UInt64(*lineage),
                units(&through.to_string()),
            ];
            values.extend(part_values(part)?);
            values.push(Value::UInt64(id.get()));
            values.push(units(&outcome.to_string()));
            values
        }
        Diagnostic::LockWaitBegan { instance, lock, id }
        | Diagnostic::LockWaitEnded { instance, lock, id } => {
            vec![
                Value::UInt64(*instance),
                units(lock),
                Value::UInt64(id.get()),
            ]
        }
        _ => return None,
    })
}
