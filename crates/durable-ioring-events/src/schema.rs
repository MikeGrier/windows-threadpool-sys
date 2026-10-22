// Copyright (c) 2026 Mike Grier
//! The events of the provider, as data (DE-D-2): what each is called, how serious it is, which keyword it belongs to, and the
//! fields it carries, in order, with their types.
//!
//! The table is the one place an event is described. The manifest is rendered from it, the sink encodes each event to it, and
//! a reader's decoder can be written from it, so the three cannot disagree about a field.

use durable_ioring::{DiagnosticKind, Edge};
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

/// A task: what a pair of events is about, which a reader groups a begin and its end by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    /// The task's name in the manifest.
    pub name: &'static str,
    /// Its value, which is an event's `EVENT_DESCRIPTOR`'s `Task`. Zero is the default and names none.
    pub value: u16,
}

/// One event of the provider.
#[derive(Clone, Copy, Debug)]
pub struct EventSchema {
    /// The event's id, which is its `EVENT_DESCRIPTOR`'s `Id` and the manifest's `value`.
    pub id: u16,
    /// The event's name in the manifest.
    pub name: &'static str,
    /// The kind of event of durable-ioring that becomes this one. A kind with several events -- a pair's begin and end, and a
    /// flush's for a file or a domain -- names them all, and they share a level, a keyword and a task.
    pub kind: DiagnosticKind,
    /// Which end of a pair the event is, as [`Diagnostic::pairing`](durable_ioring::Diagnostic::pairing) says; `None` for an
    /// event that is no end of one. It decides the event's opcode.
    pub edge: Option<Edge>,
    /// The task a pair's events share; `None` for an event that is no end of one.
    pub task: Option<&'static Task>,
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
            opcode: opcode(self.edge),
            task: match self.task {
                Some(task) => task.value,
                None => NO_TASK,
            },
            keyword: self.keyword.mask,
        }
    }
}

/// The opcode an event is written with: a pair's begin is `win:Start` and its end `win:Stop`, which a reader pairs by task and
/// activity, and any other event `win:Info`.
#[must_use]
pub const fn opcode(edge: Option<Edge>) -> u8 {
    match edge {
        None => opcode::INFO,
        Some(Edge::Begin) => opcode::START,
        Some(Edge::End) => opcode::STOP,
    }
}

/// The opcodes the provider writes, as `winmeta.xml` defines them.
pub mod opcode {
    /// `win:Info`: "Most events use INFO (0)."
    pub const INFO: u8 = 0;
    /// `win:Start`: the beginning of an activity.
    pub const START: u8 = 1;
    /// `win:Stop`: the end of an activity.
    pub const STOP: u8 = 2;
}

/// Every event is version 0: this is the first definition, and a change to an event's fields is a new version (DE-D-2).
pub const VERSION: u8 = 0;
/// "Manifest-based events normally use channel 0."
const CHANNEL: u8 = 0;
/// "The task code 0 is the default."
const NO_TASK: u16 = 0;

/// A seal, from its being asked for to its answer.
pub static SEAL_TASK: Task = Task {
    name: "Seal",
    value: 1,
};
/// One part of a seal's flush, from its issue to its answer.
pub static FLUSH_TASK: Task = Task {
    name: "Flush",
    value: 2,
};
/// A wait for one of an instance's locks.
pub static LOCK_WAIT_TASK: Task = Task {
    name: "LockWait",
    value: 3,
};

/// Waits for the instance's locks.
pub static LOCKS: Keyword = Keyword {
    name: "Locks",
    mask: 0x1,
};
/// Each part of a seal's flush.
pub static FLUSHES: Keyword = Keyword {
    name: "Flushes",
    mask: 0x2,
};
/// Each seal.
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
const FILE: Column = column("File", ColumnType::UInt64);
const DOMAIN_LENGTH: Column = column("DomainLength", ColumnType::UInt32);
const DOMAIN: Column = column(
    "Domain",
    ColumnType::Binary {
        length: "DomainLength",
    },
);
const LOCK: Column = column("Lock", ColumnType::UnicodeString);
const PAIR: Column = column("Pair", ColumnType::UInt64);
const OUTCOME: Column = column("Outcome", ColumnType::UnicodeString);

/// The id of each event. Changing one is a breaking change to every reader that decodes by it. Ids 2 to 6 were the delay events
/// that preceded the pairs, and are not used again, so a trace recorded with them is never read as one of these (DE-D-5).
pub mod id {
    /// An instance found its own state inconsistent.
    pub const INCONSISTENCY: u16 = 1;
    /// A seal was asked for.
    pub const SEAL_BEGAN: u16 = 7;
    /// A seal was answered, or never will be.
    pub const SEAL_ENDED: u16 = 8;
    /// A file's flush was issued.
    pub const FLUSH_BEGAN_FILE: u16 = 9;
    /// A file's flush answered, or never will.
    pub const FLUSH_ENDED_FILE: u16 = 10;
    /// A domain's part of a flush was issued.
    pub const FLUSH_BEGAN_DOMAIN: u16 = 11;
    /// A domain's part of a flush answered, or never will.
    pub const FLUSH_ENDED_DOMAIN: u16 = 12;
    /// A wait for one of an instance's locks began.
    pub const LOCK_WAIT_BEGAN: u16 = 13;
    /// The wait took the lock.
    pub const LOCK_WAIT_ENDED: u16 = 14;
}

/// One end of a pair: its id, name, kind, end and columns, at the information level, with its kind's keyword and task.
const fn pair_end(
    id: u16,
    name: &'static str,
    kind: DiagnosticKind,
    edge: Edge,
    keyword: &'static Keyword,
    task: &'static Task,
    columns: &'static [Column],
) -> EventSchema {
    EventSchema {
        id,
        name,
        kind,
        edge: Some(edge),
        task: Some(task),
        level: level::INFO,
        keyword,
        columns,
    }
}

/// The events of the provider, in the order of their ids.
pub static EVENTS: [EventSchema; 9] = [
    EventSchema {
        id: id::INCONSISTENCY,
        name: "Inconsistency",
        kind: DiagnosticKind::Inconsistency,
        edge: None,
        task: None,
        level: level::ERROR,
        keyword: &INCONSISTENCIES,
        columns: &[
            INSTANCE,
            column("What", ColumnType::UnicodeString),
            column("Detail", ColumnType::UnicodeString),
        ],
    },
    pair_end(
        id::SEAL_BEGAN,
        "SealBegan",
        DiagnosticKind::Seal,
        Edge::Begin,
        &SEALS,
        &SEAL_TASK,
        &[INSTANCE, LINEAGE, THROUGH, PAIR],
    ),
    pair_end(
        id::SEAL_ENDED,
        "SealEnded",
        DiagnosticKind::Seal,
        Edge::End,
        &SEALS,
        &SEAL_TASK,
        &[INSTANCE, LINEAGE, THROUGH, PAIR, OUTCOME],
    ),
    pair_end(
        id::FLUSH_BEGAN_FILE,
        "FlushBeganFile",
        DiagnosticKind::Flush,
        Edge::Begin,
        &FLUSHES,
        &FLUSH_TASK,
        &[INSTANCE, LINEAGE, THROUGH, FILE, PAIR],
    ),
    pair_end(
        id::FLUSH_ENDED_FILE,
        "FlushEndedFile",
        DiagnosticKind::Flush,
        Edge::End,
        &FLUSHES,
        &FLUSH_TASK,
        &[INSTANCE, LINEAGE, THROUGH, FILE, PAIR, OUTCOME],
    ),
    pair_end(
        id::FLUSH_BEGAN_DOMAIN,
        "FlushBeganDomain",
        DiagnosticKind::Flush,
        Edge::Begin,
        &FLUSHES,
        &FLUSH_TASK,
        &[INSTANCE, LINEAGE, THROUGH, DOMAIN_LENGTH, DOMAIN, PAIR],
    ),
    pair_end(
        id::FLUSH_ENDED_DOMAIN,
        "FlushEndedDomain",
        DiagnosticKind::Flush,
        Edge::End,
        &FLUSHES,
        &FLUSH_TASK,
        &[
            INSTANCE,
            LINEAGE,
            THROUGH,
            DOMAIN_LENGTH,
            DOMAIN,
            PAIR,
            OUTCOME,
        ],
    ),
    pair_end(
        id::LOCK_WAIT_BEGAN,
        "LockWaitBegan",
        DiagnosticKind::LockWait,
        Edge::Begin,
        &LOCKS,
        &LOCK_WAIT_TASK,
        &[INSTANCE, LOCK, PAIR],
    ),
    pair_end(
        id::LOCK_WAIT_ENDED,
        "LockWaitEnded",
        DiagnosticKind::LockWait,
        Edge::End,
        &LOCKS,
        &LOCK_WAIT_TASK,
        &[INSTANCE, LOCK, PAIR],
    ),
];

/// The tasks of the provider.
pub static TASKS: [&Task; 3] = [&SEAL_TASK, &FLUSH_TASK, &LOCK_WAIT_TASK];

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
