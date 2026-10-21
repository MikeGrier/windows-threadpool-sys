// Copyright (c) 2026 Mike Grier
//! `EVENT_DESCRIPTOR`: the metadata of an event.
//!
//! Read 2026-10-20 from the structure's page (`ms.date` 2018-12-05, `updated_at` 2024-02-22).

use windows_sys::Win32::System::Diagnostics::Etw::EVENT_DESCRIPTOR;

/// The level values the page for the structure defines, and the ranges around them.
///
/// The page: "Level values 0 through 5 are defined by Microsoft (see `evntrace.h` and `winmeta.h`). Level values 6
/// through 15 are reserved. Level values 16 through 255 can be defined by the event provider." A session that sets a
/// level filter "will only accept events where `eventDescriptor.Level <= session.LevelFilter`".
pub mod level {
    /// "Event bypasses level-based event filtering. Events should not use this level."
    pub const LOG_ALWAYS: u8 = 0;
    /// "Critical error".
    pub const CRITICAL: u8 = 1;
    /// "Error".
    pub const ERROR: u8 = 2;
    /// "Warning".
    pub const WARNING: u8 = 3;
    /// "Informational".
    pub const INFO: u8 = 4;
    /// "Verbose".
    pub const VERBOSE: u8 = 5;
}

/// The information (metadata) about an event that `EventWrite` and `EventEnabled` take.
///
/// The page for `EventWrite` says that "ProviderId, Level and Keyword are the primary means for filtering events" and
/// "Always assign a nonzero level and keyword to every event"; this crate passes whatever it is given.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EventDescriptor {
    /// "A 16-bit number used to identify manifest-based events." With the provider's decode GUID and the version it
    /// "should uniquely identify an event".
    pub id: u16,
    /// "An 8-bit number used to specify the version of a manifest-based event."
    pub version: u8,
    /// "An 8-bit number used to enable special event processing." Manifest-based events normally use 0; values below
    /// 16 are reserved for Microsoft.
    pub channel: u8,
    /// "An 8-bit number used to describe an event's severity or importance": see [`level`].
    pub level: u8,
    /// "An 8-bit number used to mark events with special semantics." Most events use 0, which is INFO.
    pub opcode: u8,
    /// "A 16-bit number used to annotate an event or related group of events." 0 is the default.
    pub task: u16,
    /// "A 64-bit bitmask used to indicate an event's membership in a set of event categories." The top 16 bits are
    /// Microsoft's and the low 48 are the provider's.
    pub keyword: u64,
}

impl EventDescriptor {
    /// The structure the calls take.
    pub(crate) const fn raw(&self) -> EVENT_DESCRIPTOR {
        EVENT_DESCRIPTOR {
            Id: self.id,
            Version: self.version,
            Channel: self.channel,
            Level: self.level,
            Opcode: self.opcode,
            Task: self.task,
            Keyword: self.keyword,
        }
    }
}
