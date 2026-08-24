// Copyright (c) 2026 Mike Grier
//! The safe `GetLogicalProcessorInformationEx` walk.
//!
//! This module is the crate's reason to exist (D-1 in `DESIGN-NOTES.md`).
//! `GetLogicalProcessorInformationEx` fills a caller-owned buffer with a
//! sequence of `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` records. Three
//! properties of that buffer make it unsafe to consume as ordinary Rust data:
//!
//! - Records are **variable length**: advance by each record's own `Size`
//!   field, never by `size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()`.
//! - Several relation types declare a trailing array as length 1
//!   (`PROCESSOR_RELATIONSHIP::GroupMask: [GROUP_AFFINITY; 1]`) while actually
//!   holding as many entries as a separate `GroupCount` field reports. Reading
//!   past element 0 through the declared array type is undefined behavior;
//!   reading past it through raw pointer arithmetic, trusting the OS's own
//!   `Size` accounting, is exactly what correct use of the API requires.
//! - The record body is a `union` discriminated by the record's
//!   `Relationship` field, unchecked by the type system.
//!
//! Everything `unsafe` in this crate is here. Every function this module
//! exposes to the rest of the crate is safe.

use std::io;

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::System::SystemInformation::{
    CACHE_RELATIONSHIP, CacheData, CacheInstruction, CacheTrace, CacheUnified, GROUP_AFFINITY,
    GROUP_RELATIONSHIP, GetLogicalProcessorInformationEx, LOGICAL_PROCESSOR_RELATIONSHIP,
    NUMA_NODE_RELATIONSHIP, PROCESSOR_GROUP_INFO, PROCESSOR_RELATIONSHIP, RelationAll,
    RelationCache, RelationGroup, RelationNumaNode, RelationNumaNodeEx, RelationProcessorCore,
    RelationProcessorDie, RelationProcessorModule, RelationProcessorPackage,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};

/// One `GROUP_AFFINITY` read from a variable-length trailing array, owned.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GroupAffinity {
    pub(crate) group: u16,
    pub(crate) mask: usize,
}

/// The decoded, owned body of a `PROCESSOR_RELATIONSHIP` record: used for
/// core, package, die, and module relationships alike, which all share this
/// shape.
#[derive(Clone, Debug)]
pub(crate) struct ProcessorBody {
    pub(crate) flags: u8,
    pub(crate) efficiency_class: u8,
    pub(crate) group_masks: Vec<GroupAffinity>,
}

/// The decoded, owned body of a `CACHE_RELATIONSHIP` record.
#[derive(Clone, Debug)]
pub(crate) struct CacheBody {
    pub(crate) level: u8,
    pub(crate) associativity: u8,
    pub(crate) line_size: u16,
    pub(crate) cache_size: u32,
    pub(crate) cache_type: i32,
    pub(crate) group_masks: Vec<GroupAffinity>,
}

/// The decoded, owned body of a `NUMA_NODE_RELATIONSHIP` record.
#[derive(Clone, Debug)]
pub(crate) struct NumaNodeBody {
    pub(crate) node_number: u32,
    pub(crate) group_masks: Vec<GroupAffinity>,
}

/// One `PROCESSOR_GROUP_INFO` entry, owned.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GroupInfo {
    pub(crate) maximum_processor_count: u8,
    pub(crate) active_processor_count: u8,
    pub(crate) active_processor_mask: usize,
}

/// The decoded, owned body of a `GROUP_RELATIONSHIP` record.
#[derive(Clone, Debug)]
pub(crate) struct GroupBody {
    pub(crate) group_info: Vec<GroupInfo>,
}

/// One decoded `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` record.
#[derive(Clone, Debug)]
pub(crate) enum Record {
    ProcessorCore(ProcessorBody),
    ProcessorPackage(ProcessorBody),
    ProcessorDie(ProcessorBody),
    ProcessorModule(ProcessorBody),
    Cache(CacheBody),
    NumaNode(NumaNodeBody),
    Group(GroupBody),
    /// A relationship kind this crate does not decode, carrying Windows's raw
    /// value so a caller can see something was present rather than silently
    /// losing it. Only read in tests and `Debug` output today; kept anyway
    /// because a future caller inspecting an unrecognised relationship is
    /// exactly this variant's purpose.
    #[allow(dead_code)]
    Unknown(LOGICAL_PROCESSOR_RELATIONSHIP),
}

/// Enumerate every logical-processor relationship the system reports.
///
/// # Errors
///
/// Returns any error from `GetLogicalProcessorInformationEx`.
pub(crate) fn enumerate() -> io::Result<Vec<Record>> {
    let mut length: u32 = 0;
    // SAFETY: a null buffer and a valid `length` out-pointer. Documented to
    // fail with `ERROR_INSUFFICIENT_BUFFER` and report the required size in
    // `length`, writing nothing through the null buffer pointer.
    let probe = unsafe {
        GetLogicalProcessorInformationEx(RelationAll, std::ptr::null_mut(), &raw mut length)
    };
    if probe != 0 {
        // Documented to fail on the sizing call; succeeding would mean zero
        // bytes were needed, i.e. nothing to report.
        return Ok(Vec::new());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(error);
    }

    // `u64`-backed storage guarantees 8-byte alignment for every record's
    // header and for the `usize`-sized fields inside its trailing arrays,
    // regardless of what a `Vec<u8>` allocation would have happened to give.
    let mut storage = vec![0_u64; length.div_ceil(8) as usize];
    let buffer = storage.as_mut_ptr().cast::<u8>();
    let mut actual_length = length;
    // SAFETY: `buffer` points to `storage`, whose byte length is at least
    // `length` (the value the sizing call just reported) and 8-byte aligned;
    // `actual_length` is a valid in/out length pointer.
    let ok = unsafe {
        GetLogicalProcessorInformationEx(RelationAll, buffer.cast(), &raw mut actual_length)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `buffer` now holds `actual_length` bytes written by the call
    // above: zero or more consecutive `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`
    // records whose `Size` fields sum to `actual_length`, per the API's own
    // contract.
    Ok(unsafe { decode(buffer.cast_const(), actual_length) })
}

const RELATIONSHIP_OFFSET: usize =
    core::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Relationship);
const SIZE_OFFSET: usize = core::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Size);
const UNION_OFFSET: usize =
    core::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);

/// Read a `T` from `base + offset`, without assuming alignment.
///
/// # Safety
///
/// `base + offset` must address at least `size_of::<T>()` initialized bytes.
unsafe fn read_at<T: Copy>(base: *const u8, offset: usize) -> T {
    // SAFETY: forwarded from the caller.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}

/// Read `count` consecutive `GROUP_AFFINITY` entries starting at `base`,
/// trusting `count` rather than any type-declared array length (see the
/// module's own documentation).
///
/// # Safety
///
/// `base` must address at least `count` consecutive, initialized
/// `GROUP_AFFINITY` values.
unsafe fn read_group_affinities(base: *const u8, count: u16) -> Vec<GroupAffinity> {
    (0..u32::from(count))
        .map(|i| {
            let offset = i as usize * size_of::<GROUP_AFFINITY>();
            // SAFETY: forwarded from the caller; `i < count`.
            let raw: GROUP_AFFINITY = unsafe { read_at(base, offset) };
            GroupAffinity {
                group: raw.Group,
                mask: raw.Mask,
            }
        })
        .collect()
}

/// As [`read_group_affinities`], for `CACHE_RELATIONSHIP`/
/// `NUMA_NODE_RELATIONSHIP`'s union-based `GroupMask`/`GroupMasks` layout.
///
/// `GroupCount` was introduced in Windows 20H2 for these two structures and
/// reads `0` on every earlier version -- not because there are no groups,
/// but because the union there still holds the legacy singular `GroupMask`
/// member rather than an empty `GroupMasks` array. Both members share the
/// same offset, so treating `group_count == 0` as "read one entry" recovers
/// that legacy affinity instead of silently decoding it as an empty set.
///
/// # Safety
///
/// `base` must address at least `group_count.max(1)` consecutive,
/// initialized `GROUP_AFFINITY` values.
unsafe fn read_legacy_group_affinities(base: *const u8, group_count: u16) -> Vec<GroupAffinity> {
    let count = group_count.max(1);
    // SAFETY: forwarded from the caller.
    unsafe { read_group_affinities(base, count) }
}

/// # Safety
///
/// `buffer` must address `length` initialized bytes written by
/// `GetLogicalProcessorInformationEx`: zero or more consecutive
/// `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` records whose `Size` fields sum
/// to `length`.
unsafe fn decode(buffer: *const u8, length: u32) -> Vec<Record> {
    let mut records = Vec::new();
    let mut offset: usize = 0;
    let length = length as usize;
    while offset < length {
        // SAFETY: `offset < length`, and the caller's contract guarantees a
        // full record header lives at this offset.
        let record_base = unsafe { buffer.add(offset) };
        // SAFETY: `record_base` addresses a full record header.
        let relationship: LOGICAL_PROCESSOR_RELATIONSHIP =
            unsafe { read_at(record_base, RELATIONSHIP_OFFSET) };
        // SAFETY: as above.
        let size: u32 = unsafe { read_at(record_base, SIZE_OFFSET) };
        assert!(
            size > 0,
            "GetLogicalProcessorInformationEx reported a zero-size record"
        );
        // SAFETY: the union starts within this record, which the caller's
        // contract guarantees is `size` bytes of initialized data.
        let union_base = unsafe { record_base.add(UNION_OFFSET) };
        // SAFETY: `union_base` addresses the union body of a record whose
        // `Relationship` field is `relationship`, and whose `Size` accounts
        // for whatever trailing array that relationship's body declares.
        records.push(unsafe { decode_body(relationship, union_base) });
        offset += size as usize;
    }
    records
}

/// # Safety
///
/// `union_base` must address the union body of a
/// `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` record whose `Relationship`
/// field is `relationship`, with enough trailing bytes for whatever variable
/// array that relationship's body declares.
unsafe fn decode_body(
    relationship: LOGICAL_PROCESSOR_RELATIONSHIP,
    union_base: *const u8,
) -> Record {
    // windows-sys names these relationship constants in mixed case, not
    // SCREAMING_CASE; that is not this crate's naming to change.
    #[allow(non_upper_case_globals)]
    match relationship {
        // SAFETY: forwarded from the caller.
        RelationProcessorCore => Record::ProcessorCore(unsafe { read_processor_body(union_base) }),
        RelationProcessorPackage => {
            // SAFETY: forwarded from the caller.
            Record::ProcessorPackage(unsafe { read_processor_body(union_base) })
        }
        RelationProcessorDie => Record::ProcessorDie(unsafe { read_processor_body(union_base) }),
        RelationProcessorModule => {
            // SAFETY: forwarded from the caller.
            Record::ProcessorModule(unsafe { read_processor_body(union_base) })
        }
        // SAFETY: forwarded from the caller.
        RelationCache => Record::Cache(unsafe { read_cache_body(union_base) }),
        // SAFETY: forwarded from the caller.
        RelationNumaNode | RelationNumaNodeEx => {
            Record::NumaNode(unsafe { read_numa_body(union_base) })
        }
        // SAFETY: forwarded from the caller.
        RelationGroup => Record::Group(unsafe { read_group_body(union_base) }),
        other => Record::Unknown(other),
    }
}

/// # Safety
///
/// `base` must address a `PROCESSOR_RELATIONSHIP` whose trailing `GroupMask`
/// array has at least `GroupCount` initialized entries.
unsafe fn read_processor_body(base: *const u8) -> ProcessorBody {
    // SAFETY: forwarded from the caller.
    let flags: u8 = unsafe { read_at(base, core::mem::offset_of!(PROCESSOR_RELATIONSHIP, Flags)) };
    // SAFETY: forwarded from the caller.
    let efficiency_class: u8 = unsafe {
        read_at(
            base,
            core::mem::offset_of!(PROCESSOR_RELATIONSHIP, EfficiencyClass),
        )
    };
    // SAFETY: forwarded from the caller.
    let group_count: u16 = unsafe {
        read_at(
            base,
            core::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupCount),
        )
    };
    // SAFETY: forwarded from the caller; `group_count` names the true length.
    let group_masks = unsafe {
        read_group_affinities(
            base.add(core::mem::offset_of!(PROCESSOR_RELATIONSHIP, GroupMask)),
            group_count,
        )
    };
    ProcessorBody {
        flags,
        efficiency_class,
        group_masks,
    }
}

/// # Safety
///
/// `base` must address a `CACHE_RELATIONSHIP` whose trailing `GroupMask`
/// array (behind its anonymous union) has at least `GroupCount.max(1)`
/// initialized entries -- pre-Windows-20H2 records report `GroupCount == 0`
/// but still have exactly one legacy `GroupMask` entry there (see
/// [`read_legacy_group_affinities`]).
unsafe fn read_cache_body(base: *const u8) -> CacheBody {
    // SAFETY: forwarded from the caller.
    let level: u8 = unsafe { read_at(base, core::mem::offset_of!(CACHE_RELATIONSHIP, Level)) };
    // SAFETY: forwarded from the caller.
    let associativity: u8 = unsafe {
        read_at(
            base,
            core::mem::offset_of!(CACHE_RELATIONSHIP, Associativity),
        )
    };
    // SAFETY: forwarded from the caller.
    let line_size: u16 =
        unsafe { read_at(base, core::mem::offset_of!(CACHE_RELATIONSHIP, LineSize)) };
    // SAFETY: forwarded from the caller.
    let cache_size: u32 =
        unsafe { read_at(base, core::mem::offset_of!(CACHE_RELATIONSHIP, CacheSize)) };
    // SAFETY: forwarded from the caller.
    let cache_type: i32 = unsafe { read_at(base, core::mem::offset_of!(CACHE_RELATIONSHIP, Type)) };
    // SAFETY: forwarded from the caller.
    let group_count: u16 =
        unsafe { read_at(base, core::mem::offset_of!(CACHE_RELATIONSHIP, GroupCount)) };
    // SAFETY: forwarded from the caller; `group_count` names the true length.
    let group_masks = unsafe {
        read_legacy_group_affinities(
            base.add(core::mem::offset_of!(CACHE_RELATIONSHIP, Anonymous)),
            group_count,
        )
    };
    CacheBody {
        level,
        associativity,
        line_size,
        cache_size,
        cache_type,
        group_masks,
    }
}

/// # Safety
///
/// `base` must address a `NUMA_NODE_RELATIONSHIP` whose trailing `GroupMask`
/// array (behind its anonymous union) has at least `GroupCount.max(1)`
/// initialized entries -- pre-Windows-20H2 records report `GroupCount == 0`
/// but still have exactly one legacy `GroupMask` entry there (see
/// [`read_legacy_group_affinities`]).
unsafe fn read_numa_body(base: *const u8) -> NumaNodeBody {
    // SAFETY: forwarded from the caller.
    let node_number: u32 = unsafe {
        read_at(
            base,
            core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, NodeNumber),
        )
    };
    // SAFETY: forwarded from the caller.
    let group_count: u16 = unsafe {
        read_at(
            base,
            core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, GroupCount),
        )
    };
    // SAFETY: forwarded from the caller; `group_count` names the true length.
    let group_masks = unsafe {
        read_legacy_group_affinities(
            base.add(core::mem::offset_of!(NUMA_NODE_RELATIONSHIP, Anonymous)),
            group_count,
        )
    };
    NumaNodeBody {
        node_number,
        group_masks,
    }
}

/// # Safety
///
/// `base` must address a `GROUP_RELATIONSHIP` whose trailing `GroupInfo`
/// array has at least `ActiveGroupCount` initialized entries.
unsafe fn read_group_body(base: *const u8) -> GroupBody {
    // SAFETY: forwarded from the caller.
    let active_group_count: u16 = unsafe {
        read_at(
            base,
            core::mem::offset_of!(GROUP_RELATIONSHIP, ActiveGroupCount),
        )
    };
    // SAFETY: forwarded from the caller.
    let info_base = unsafe { base.add(core::mem::offset_of!(GROUP_RELATIONSHIP, GroupInfo)) };
    let group_info = (0..u32::from(active_group_count))
        .map(|i| {
            let offset = i as usize * size_of::<PROCESSOR_GROUP_INFO>();
            // SAFETY: forwarded from the caller; `i < active_group_count`.
            let raw: PROCESSOR_GROUP_INFO = unsafe { read_at(info_base, offset) };
            GroupInfo {
                maximum_processor_count: raw.MaximumProcessorCount,
                active_processor_count: raw.ActiveProcessorCount,
                active_processor_mask: raw.ActiveProcessorMask,
            }
        })
        .collect();
    GroupBody { group_info }
}

/// What a cache holds, converted from Windows's raw `PROCESSOR_CACHE_TYPE`.
// windows-sys names these cache-type constants in mixed case, not
// SCREAMING_CASE; that is not this crate's naming to change.
#[allow(non_upper_case_globals)]
pub(crate) fn cache_kind_from_raw(value: i32) -> crate::CacheKind {
    match value {
        CacheUnified => crate::CacheKind::Unified,
        CacheInstruction => crate::CacheKind::Instruction,
        CacheData => crate::CacheKind::Data,
        CacheTrace => crate::CacheKind::Trace,
        other => crate::CacheKind::Other(other),
    }
}

#[cfg(test)]
mod tests;
