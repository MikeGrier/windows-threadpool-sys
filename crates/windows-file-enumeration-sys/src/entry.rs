// Copyright (c) 2026 Mike Grier
//! What one enumerated directory entry carries.
//!
//! Every field a `FILE_ID_EXTD_DIR_INFO` record supplies inline is present on
//! every entry, in the unit the record reported it. That is not generosity: the
//! record pays for all of them in the same query, so making any of them optional
//! would save no native work while narrowing the platform.
//!
//! The one thing that *is* selectable is volume qualification
//! ([`FileIdentityMode`]), because obtaining a volume serial needs a second
//! query against the directory handle.
//!
//! `FileIndex` is deliberately absent. Windows documents it as undefined for
//! filesystems including NTFS, so exposing it would invite callers to depend on
//! a value with no meaning.

use wtf_string::{Wtf16Str, Wtf16String};

use crate::WindowsFileTimestamp;

/// Whether an entry is a directory or an ordinary file.
///
/// This is closed rather than extensible because Windows decides it with one
/// attribute bit: an entry either has `FILE_ATTRIBUTE_DIRECTORY` or it does not.
/// Anything finer -- a reparse point, a device, an offline file -- is a property
/// of the raw attributes, which every entry also carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntryType {
    /// The entry is not a directory.
    File,
    /// The entry is a directory.
    Directory,
}

/// A filesystem object's identity: the record's 128-bit file ID, optionally
/// qualified by the volume it lives on.
///
/// A file ID is unique only *within* a volume, so the same ID may name different
/// objects on different volumes. An unqualified identity is therefore not
/// globally meaningful and must not be compared across volumes;
/// [`is_volume_qualified`](Self::is_volume_qualified) reports which kind this is.
///
/// The 16 identifier bytes are kept exactly as the record reported them. They
/// are deliberately not folded into a `u128`, which would impose an endianness
/// the native value does not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    id: [u8; 16],
    volume_serial: Option<u64>,
}

impl FileIdentity {
    /// Build an identity from a record's identifier bytes and, when it was
    /// obtained, the volume serial that qualifies them.
    #[must_use]
    pub const fn new(id: [u8; 16], volume_serial: Option<u64>) -> Self {
        Self { id, volume_serial }
    }

    /// The record's 16 identifier bytes, verbatim.
    #[must_use]
    pub const fn id_bytes(&self) -> [u8; 16] {
        self.id
    }

    /// The volume serial, when the request obtained one.
    #[must_use]
    pub const fn volume_serial(&self) -> Option<u64> {
        self.volume_serial
    }

    /// Whether this identity is globally meaningful.
    #[must_use]
    pub const fn is_volume_qualified(&self) -> bool {
        self.volume_serial.is_some()
    }
}

/// How much work a request is willing to do for file identity.
///
/// The 128-bit file ID is inline in every record and always present. Only the
/// volume serial that qualifies it costs an extra query, and that query runs
/// once against the directory handle -- no mode ever opens an individual entry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FileIdentityMode {
    /// Do not query the volume serial. Entries carry unqualified identities.
    ///
    /// The default, because a caller that never compares identities across
    /// volumes should not pay for the query.
    #[default]
    Omit,
    /// Query the volume serial once, and carry on without it if that fails.
    ///
    /// Entries then carry unqualified identities rather than failing the
    /// enumeration -- the shape a caller wants when identity refines its work
    /// but does not gate it.
    BestEffort,
    /// Query the volume serial once, and fail the enumeration before its first
    /// entry if it cannot be obtained.
    ///
    /// Use this when an unqualified identity would be silently wrong.
    Required,
}

impl FileIdentityMode {
    /// Whether this mode performs the volume-serial query at all.
    #[must_use]
    pub const fn queries_volume(self) -> bool {
        matches!(
            self,
            FileIdentityMode::BestEffort | FileIdentityMode::Required
        )
    }
}

/// One enumerated directory entry with its full inline metadata.
///
/// Names are the entry's own leaf name -- never a path -- and stay native-width
/// WTF-16, so an ill-formed surrogate a filesystem happens to contain survives
/// the round trip rather than being replaced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryEntry {
    name: Wtf16String,
    attributes: u32,
    reparse_tag: Option<u32>,
    logical_size: u64,
    allocation_size: u64,
    extended_attribute_size: u32,
    creation_time: WindowsFileTimestamp,
    last_access_time: WindowsFileTimestamp,
    last_write_time: WindowsFileTimestamp,
    change_time: WindowsFileTimestamp,
    identity: FileIdentity,
}

/// The parsed field values of one native record, before they become a
/// [`DirectoryEntry`].
///
/// This exists so the native engine can hand over a dozen values without a
/// dozen positional arguments, and so adding a field later is not a breaking
/// change to a constructor. It is crate-internal: the public surface is
/// [`DirectoryEntry`]'s accessors.
pub(crate) struct EntryFields {
    pub(crate) name: Wtf16String,
    pub(crate) attributes: u32,
    pub(crate) logical_size: u64,
    pub(crate) allocation_size: u64,
    pub(crate) extended_attribute_size: u32,
    pub(crate) creation_time: WindowsFileTimestamp,
    pub(crate) last_access_time: WindowsFileTimestamp,
    pub(crate) last_write_time: WindowsFileTimestamp,
    pub(crate) change_time: WindowsFileTimestamp,
    pub(crate) reparse_tag: u32,
    pub(crate) identity: FileIdentity,
}

/// `FILE_ATTRIBUTE_DIRECTORY`, the single bit that decides [`EntryType`].
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;

/// `FILE_ATTRIBUTE_REPARSE_POINT`, the single bit that decides whether the
/// record's reparse tag means anything.
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

impl DirectoryEntry {
    /// Build an entry from one record's parsed fields.
    ///
    /// The reparse tag is admitted only when the attributes say the entry is a
    /// reparse point. A record's tag field is otherwise meaningless, and
    /// surfacing it would let a caller act on a tag that names nothing.
    #[must_use]
    pub(crate) fn from_fields(fields: EntryFields) -> Self {
        let reparse_tag =
            (fields.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0).then_some(fields.reparse_tag);
        Self {
            name: fields.name,
            attributes: fields.attributes,
            reparse_tag,
            logical_size: fields.logical_size,
            allocation_size: fields.allocation_size,
            extended_attribute_size: fields.extended_attribute_size,
            creation_time: fields.creation_time,
            last_access_time: fields.last_access_time,
            last_write_time: fields.last_write_time,
            change_time: fields.change_time,
            identity: fields.identity,
        }
    }

    /// The entry's own leaf name, in native WTF-16.
    #[must_use]
    pub fn name(&self) -> &Wtf16Str {
        &self.name
    }

    /// Take ownership of the name, consuming the entry.
    #[must_use]
    pub fn into_name(self) -> Wtf16String {
        self.name
    }

    /// Whether the entry is a directory or a file.
    #[must_use]
    pub const fn entry_type(&self) -> EntryType {
        if self.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            EntryType::Directory
        } else {
            EntryType::File
        }
    }

    /// The raw `FILE_ATTRIBUTE_*` bitmask, exactly as the record reported it.
    #[must_use]
    pub const fn attributes(&self) -> u32 {
        self.attributes
    }

    /// Whether the entry is a reparse point.
    #[must_use]
    pub const fn is_reparse_point(&self) -> bool {
        self.reparse_tag.is_some()
    }

    /// The reparse tag, present exactly when the entry is a reparse point.
    #[must_use]
    pub const fn reparse_tag(&self) -> Option<u32> {
        self.reparse_tag
    }

    /// The end-of-file offset in bytes: how much data the entry holds.
    #[must_use]
    pub const fn logical_size(&self) -> u64 {
        self.logical_size
    }

    /// The bytes allocated on the volume, which may exceed or -- for a
    /// compressed or sparse file -- fall short of the logical size.
    #[must_use]
    pub const fn allocation_size(&self) -> u64 {
        self.allocation_size
    }

    /// The size of the entry's extended attributes, in bytes.
    #[must_use]
    pub const fn extended_attribute_size(&self) -> u32 {
        self.extended_attribute_size
    }

    /// When the entry was created.
    #[must_use]
    pub const fn creation_time(&self) -> WindowsFileTimestamp {
        self.creation_time
    }

    /// When the entry was last accessed.
    #[must_use]
    pub const fn last_access_time(&self) -> WindowsFileTimestamp {
        self.last_access_time
    }

    /// When the entry's data was last written.
    #[must_use]
    pub const fn last_write_time(&self) -> WindowsFileTimestamp {
        self.last_write_time
    }

    /// When the entry's metadata last changed.
    ///
    /// This has no `WIN32_FIND_DATAW` equivalent; it is one of the fields that
    /// makes the extended directory-information class worth requiring.
    #[must_use]
    pub const fn change_time(&self) -> WindowsFileTimestamp {
        self.change_time
    }

    /// The entry's identity, volume-qualified only if the request asked for it.
    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }
}

#[cfg(test)]
mod tests;
