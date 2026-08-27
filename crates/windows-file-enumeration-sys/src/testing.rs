// Copyright (c) 2026 Mike Grier
//! Test-only helpers for building the values the native engine will later
//! produce.
//!
//! Predicate evaluation and entry metadata are testable long before there is a
//! directory handle to read them from, so this builds a [`DirectoryEntry`]
//! directly from field values. It exists only under `cfg(test)`.

use wtf_string::Wtf16String;

use crate::entry::{DirectoryEntry, EntryFields, FileIdentity};
use crate::timestamp::WindowsFileTimestamp;

/// `FILE_ATTRIBUTE_DIRECTORY`.
pub(crate) const ATTR_DIRECTORY: u32 = 0x0000_0010;

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
pub(crate) const ATTR_REPARSE_POINT: u32 = 0x0000_0400;

/// `FILE_ATTRIBUTE_READONLY`.
pub(crate) const ATTR_READONLY: u32 = 0x0000_0001;

/// `FILE_ATTRIBUTE_HIDDEN`.
pub(crate) const ATTR_HIDDEN: u32 = 0x0000_0002;

/// A builder for the entries a parser will later hand over.
pub(crate) struct EntryBuilder {
    fields: EntryFields,
}

impl EntryBuilder {
    /// An ordinary zero-sized file with the given name and no attributes.
    pub(crate) fn file(name: &str) -> Self {
        Self {
            fields: EntryFields {
                name: Wtf16String::from(name),
                attributes: 0,
                logical_size: 0,
                allocation_size: 0,
                extended_attribute_size: 0,
                creation_time: WindowsFileTimestamp::ZERO,
                last_access_time: WindowsFileTimestamp::ZERO,
                last_write_time: WindowsFileTimestamp::ZERO,
                change_time: WindowsFileTimestamp::ZERO,
                reparse_tag: 0,
                identity: FileIdentity::new([0; 16], None),
            },
        }
    }

    /// A file whose name is given as raw code units, so a test can use one a
    /// `str` cannot express.
    pub(crate) fn file_units(units: &[u16]) -> Self {
        let mut builder = Self::file("");
        builder.fields.name = Wtf16String::from_units(units);
        builder
    }

    pub(crate) fn attributes(mut self, attributes: u32) -> Self {
        self.fields.attributes = attributes;
        self
    }

    pub(crate) fn reparse(mut self, tag: u32) -> Self {
        self.fields.attributes |= ATTR_REPARSE_POINT;
        self.fields.reparse_tag = tag;
        self
    }

    /// Set the reparse tag *without* the attribute bit, to prove the entry
    /// suppresses a tag the attributes do not justify.
    pub(crate) fn bare_reparse_tag(mut self, tag: u32) -> Self {
        self.fields.reparse_tag = tag;
        self
    }

    pub(crate) fn logical_size(mut self, bytes: u64) -> Self {
        self.fields.logical_size = bytes;
        self
    }

    pub(crate) fn allocation_size(mut self, bytes: u64) -> Self {
        self.fields.allocation_size = bytes;
        self
    }

    pub(crate) fn extended_attribute_size(mut self, bytes: u32) -> Self {
        self.fields.extended_attribute_size = bytes;
        self
    }

    pub(crate) fn times(
        mut self,
        creation: i64,
        last_access: i64,
        last_write: i64,
        change: i64,
    ) -> Self {
        self.fields.creation_time = WindowsFileTimestamp::from_ticks(creation);
        self.fields.last_access_time = WindowsFileTimestamp::from_ticks(last_access);
        self.fields.last_write_time = WindowsFileTimestamp::from_ticks(last_write);
        self.fields.change_time = WindowsFileTimestamp::from_ticks(change);
        self
    }

    pub(crate) fn identity(mut self, identity: FileIdentity) -> Self {
        self.fields.identity = identity;
        self
    }

    pub(crate) fn build(self) -> DirectoryEntry {
        DirectoryEntry::from_fields(self.fields)
    }
}

/// A file entry with just a name, which most predicate tests want.
pub(crate) fn named_file(name: &str) -> DirectoryEntry {
    EntryBuilder::file(name).build()
}

/// A directory entry with just a name.
pub(crate) fn named_directory(name: &str) -> DirectoryEntry {
    EntryBuilder::file(name).attributes(ATTR_DIRECTORY).build()
}
