// Copyright (c) 2026 Mike Grier
//! The events of the provider, as data (DE-D-2): what each is called, how serious it is, which keyword it belongs to, and the
//! fields it carries, in order, with their types.
//!
//! The table is the one place an event is described. The manifest is rendered from it, the sink encodes each event to it, and
//! a reader's decoder can be written from it, so the three cannot disagree about a field.

use durable_ioring::DiagnosticKind;
use win_etw_sys::{EventDescriptor, Guid, level};

#[cfg(test)]
mod tests;

/// The provider's name, which is the crate's.
pub const PROVIDER_NAME: &str = "durable-ioring-events";

/// The provider's GUID, generated once on 2026-10-20.
///
/// Changing it breaks every session that enabled the provider by it, which is what a GUID identifies; so it is not changed.
pub const PROVIDER_ID: Guid = Guid::from_u128(0x4706_7f6f_8f2b_4278_9b92_092d_fb33_85e6);

/// What a field is, as a manifest names it and as it is laid out in the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColumnType {
    /// `win:UInt32`: four bytes.
    UInt32,
    /// `win:UInt64`: eight bytes.
    UInt64,
    /// `win:Boolean`: four bytes, one for true and zero for false.
    Boolean,
    /// `win:UnicodeString`: UTF-16 units and then a zero unit. A zero unit in the text ends it for a reader.
    UnicodeString,
    /// `win:Binary`: as many bytes as the `UInt32` column of this name, earlier in the same event, says.
    Binary {
        /// The name of that column.
        length: &'static str,
    },
}

/// One field of an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Column {
    /// The field's name in the manifest.
    pub name: &'static str,
    /// What it is.
    pub ty: ColumnType,
}

/// A keyword: the category an event belongs to, which a session filters by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyword {
    /// The keyword's name in the manifest.
    pub name: &'static str,
    /// Its bit. The top sixteen bits of a keyword are Microsoft's, so these are in the low forty-eight.
    pub mask: u64,
}

/// One event of the provider.
#[derive(Clone, Copy, Debug)]
pub struct EventSchema {
    /// The event's id, which is its `EVENT_DESCRIPTOR`'s `Id` and the manifest's `value`.
    pub id: u16,
    /// The event's name in the manifest.
    pub name: &'static str,
    /// The kind of event of durable-ioring that becomes this one. A kind with two events, a flush answered by a file or by a
    /// domain, names both, and they share a level and a keyword.
    pub kind: DiagnosticKind,
    /// The level: see [`win_etw_sys::level`].
    pub level: u8,
    /// The keyword.
    pub keyword: &'static Keyword,
    /// The fields, in the order they are written.
    pub columns: &'static [Column],
}

impl EventSchema {
    /// The event's descriptor, as it is written and as a session filters by it.
    #[must_use]
    pub const fn descriptor(&self) -> EventDescriptor {
        EventDescriptor {
            id: self.id,
            version: VERSION,
            channel: CHANNEL,
            level: self.level,
            opcode: OPCODE,
            task: TASK,
            keyword: self.keyword.mask,
        }
    }
}

/// Every event is version 0: this is the first definition, and a change to an event's fields is a new version (DE-D-2).
pub const VERSION: u8 = 0;
/// "Manifest-based events normally use channel 0."
const CHANNEL: u8 = 0;
/// "Most events use INFO (0)."
const OPCODE: u8 = 0;
/// "The task code 0 is the default."
const TASK: u16 = 0;

/// Waits for the instance's locks.
pub static LOCKS: Keyword = Keyword {
    name: "Locks",
    mask: 0x1,
};
/// A seal's flushes, and how long each took.
pub static FLUSHES: Keyword = Keyword {
    name: "Flushes",
    mask: 0x2,
};
/// A seal waiting for the writes at or below it.
pub static SEALS: Keyword = Keyword {
    name: "Seals",
    mask: 0x4,
};
/// An instance finding its own state inconsistent.
pub static INCONSISTENCIES: Keyword = Keyword {
    name: "Inconsistencies",
    mask: 0x8,
};

const fn column(name: &'static str, ty: ColumnType) -> Column {
    Column { name, ty }
}

const INSTANCE: Column = column("Instance", ColumnType::UInt64);
const LINEAGE: Column = column("Lineage", ColumnType::UInt64);
const THROUGH: Column = column("Through", ColumnType::UnicodeString);
const ELAPSED: Column = column("ElapsedNanoseconds", ColumnType::UInt64);
const SUCCEEDED: Column = column("Succeeded", ColumnType::Boolean);

/// The id of each event. Changing one is a breaking change to every reader that decodes by it.
pub mod id {
    /// An instance found its own state inconsistent.
    pub const INCONSISTENCY: u16 = 1;
    /// A wait for one of an instance's locks that was not immediate.
    pub const LOCK_WAIT: u16 = 2;
    /// A seal's flushes, and its request to the consumer's provider, were issued.
    pub const FLUSH_ISSUED: u16 = 3;
    /// A flush of a file answered.
    pub const FLUSH_ANSWERED_FILE: u16 = 4;
    /// A flush of a domain answered.
    pub const FLUSH_ANSWERED_DOMAIN: u16 = 5;
    /// A seal that waited for the writes at or below it is issued.
    pub const SEAL_WAITED: u16 = 6;
}

/// The events of the provider, in the order of their ids.
pub static EVENTS: [EventSchema; 6] = [
    EventSchema {
        id: id::INCONSISTENCY,
        name: "Inconsistency",
        kind: DiagnosticKind::Inconsistency,
        level: level::ERROR,
        keyword: &INCONSISTENCIES,
        columns: &[
            INSTANCE,
            column("What", ColumnType::UnicodeString),
            column("Detail", ColumnType::UnicodeString),
        ],
    },
    EventSchema {
        id: id::LOCK_WAIT,
        name: "LockWait",
        kind: DiagnosticKind::LockWait,
        level: level::INFO,
        keyword: &LOCKS,
        columns: &[
            INSTANCE,
            column("Lock", ColumnType::UnicodeString),
            column("WaitedNanoseconds", ColumnType::UInt64),
        ],
    },
    EventSchema {
        id: id::FLUSH_ISSUED,
        name: "FlushIssued",
        kind: DiagnosticKind::FlushIssued,
        level: level::INFO,
        keyword: &FLUSHES,
        columns: &[
            INSTANCE,
            LINEAGE,
            THROUGH,
            column("Files", ColumnType::UInt64),
            column("Domains", ColumnType::UInt64),
        ],
    },
    EventSchema {
        id: id::FLUSH_ANSWERED_FILE,
        name: "FlushAnsweredFile",
        kind: DiagnosticKind::FlushAnswered,
        level: level::INFO,
        keyword: &FLUSHES,
        columns: &[
            INSTANCE,
            LINEAGE,
            THROUGH,
            column("File", ColumnType::UInt64),
            ELAPSED,
            SUCCEEDED,
        ],
    },
    EventSchema {
        id: id::FLUSH_ANSWERED_DOMAIN,
        name: "FlushAnsweredDomain",
        kind: DiagnosticKind::FlushAnswered,
        level: level::INFO,
        keyword: &FLUSHES,
        columns: &[
            INSTANCE,
            LINEAGE,
            THROUGH,
            column("DomainLength", ColumnType::UInt32),
            column(
                "Domain",
                ColumnType::Binary {
                    length: "DomainLength",
                },
            ),
            ELAPSED,
            SUCCEEDED,
        ],
    },
    EventSchema {
        id: id::SEAL_WAITED,
        name: "SealWaited",
        kind: DiagnosticKind::SealWaited,
        level: level::INFO,
        keyword: &SEALS,
        columns: &[
            INSTANCE,
            LINEAGE,
            THROUGH,
            column("WaitedNanoseconds", ColumnType::UInt64),
        ],
    },
];

/// The keywords of the provider.
pub static KEYWORDS: [&Keyword; 4] = [&LOCKS, &FLUSHES, &SEALS, &INCONSISTENCIES];

/// The event a kind of event of durable-ioring is asked about by: the first of its events, and its level and keyword are the
/// others' too. `None` for a kind this crate does not know, which a newer durable-ioring may add.
#[must_use]
pub fn schema_of_kind(kind: DiagnosticKind) -> Option<&'static EventSchema> {
    EVENTS.iter().find(|event| event.kind == kind)
}

/// The event with this id.
#[must_use]
pub fn schema_of_id(id: u16) -> Option<&'static EventSchema> {
    EVENTS.iter().find(|event| event.id == id)
}
