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
//! - The buffer is sized by one call and filled by another, so the machine can
//!   grow in between and make the second call fail for the same reason the
//!   first did. The pair is therefore attempted more than once -- see
//!   [`SIZING_ATTEMPTS`](crate::records::SIZING_ATTEMPTS).
//!
//! Everything `unsafe` in this crate is here. Every function this module
//! exposes to the rest of the crate is safe.
//!
//! ## How the records are walked
//!
//! Through [`crate::records`], which both of this crate's enumerations share.
//! Per [D-24](../DESIGN-NOTES.md#d-24) the operating system is **trusted** for
//! the structural validity of a buffer it just wrote -- this is not a trust
//! boundary and there is no validation pass. The walk bounds its reads because
//! bounds are how it knows where a record ends, which is a decoding
//! requirement rather than a defence.
//!
//! Two consequences a reader should not have to infer: nothing here **panics**
//! over the shape of the buffer, because a malformed record is not evidence
//! that this crate reached an inconsistent state; and a record that cannot be
//! decoded is **recorded** in
//! [`MachineMemoryTopology::enumeration_anomalies`](crate::MachineMemoryTopology::enumeration_anomalies)
//! rather than silently dropped, so a short list is distinguishable from a
//! small machine.

use crate::EnumerationAnomaly;
use crate::observation::Source;
use crate::records::{Record as RawRecord, RecordWalk};
use std::io;

use crate::records::SIZING_ATTEMPTS;
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
pub(crate) fn enumerate() -> io::Result<(Vec<Record>, Vec<EnumerationAnomaly>)> {
    // **Sized and fetched in a bounded loop**, because the machine can change
    // between the two calls -- see `SIZING_ATTEMPTS` for why a second
    // `ERROR_INSUFFICIENT_BUFFER` is retried rather than returned.
    let mut grew = None;
    for _ in 0..SIZING_ATTEMPTS {
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
            return Ok((Vec::new(), Vec::new()));
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
        if ok != 0 {
            // SAFETY: `buffer` now holds `actual_length` bytes written by the
            // call above: zero or more consecutive
            // `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` records whose `Size`
            // fields sum to `actual_length`, per the API's own contract.
            return Ok(unsafe { decode(buffer.cast_const(), actual_length) });
        }

        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error);
        }
        // The machine grew between the sizing call and this one. Size again.
        grew = Some(error);
    }

    Err(grew.expect("SIZING_ATTEMPTS is a non-zero constant, so the loop ran at least once"))
}
const RELATIONSHIP_OFFSET: usize =
    core::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Relationship);
const SIZE_OFFSET: usize = core::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Size);
const UNION_OFFSET: usize =
    core::mem::offset_of!(SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, Anonymous);

/// Read `count` consecutive `GROUP_AFFINITY` entries starting at `base`,
/// trusting `count` rather than any type-declared array length (see the
/// module's own documentation).
///
/// # Safety
///
/// `base` must address at least `count` consecutive, initialized
/// `GROUP_AFFINITY` values.
unsafe fn read_group_affinities(
    record: RawRecord,
    at: usize,
    count: u16,
) -> (Vec<GroupAffinity>, bool) {
    // SAFETY: forwarded from the caller. The read is bounded by the record's
    // own `Size`, so a `count` larger than the record can hold yields only the
    // entries that fit -- which is what closes the amplification this `u16`
    // used to have over a 16-byte stride (D-24).
    let (raw, complete) = unsafe { record.read_array::<GROUP_AFFINITY>(at, usize::from(count)) };
    let affinities = raw
        .into_iter()
        .map(|entry| GroupAffinity {
            group: entry.Group,
            mask: entry.Mask,
        })
        .collect();
    (affinities, complete)
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
unsafe fn read_legacy_group_affinities(
    record: RawRecord,
    at: usize,
    group_count: u16,
) -> (Vec<GroupAffinity>, bool) {
    let count = group_count.max(1);
    // SAFETY: forwarded from the caller.
    unsafe { read_group_affinities(record, at, count) }
}

/// # Safety
///
/// `buffer` must address `length` initialized bytes written by
/// `GetLogicalProcessorInformationEx`: zero or more consecutive
/// `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX` records whose `Size` fields sum
/// to `length`.
unsafe fn decode(buffer: *const u8, length: u32) -> (Vec<Record>, Vec<EnumerationAnomaly>) {
    // The minimum is the fixed header -- `Relationship` and `Size` -- and
    // deliberately **not** `size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>()`.
    // That struct is 80 bytes because its union is as large as its largest arm
    // (`GROUP_RELATIONSHIP`, 72), while a real processor-core record is 8 + 40 =
    // 48. Using the struct size would reject every processor, cache and NUMA
    // record on every machine. Measured, not assumed.
    //
    // Each body then bounds its own reads against the record's own `Size`, so
    // a record too short for the body it claims yields no body rather than
    // reading into its neighbour.
    // SAFETY: forwarded from this function's own contract.
    let mut walk = unsafe {
        RecordWalk::new(
            buffer,
            length,
            SIZE_OFFSET,
            UNION_OFFSET,
            Source::RelationshipWalk,
        )
    };

    let mut records = Vec::new();
    let mut anomalies = Vec::new();
    for raw in &mut walk {
        // SAFETY: the walk proved the header is within the record.
        let Some(relationship) =
            (unsafe { raw.read::<LOGICAL_PROCESSOR_RELATIONSHIP>(RELATIONSHIP_OFFSET) })
        else {
            continue;
        };
        // SAFETY: `raw` addresses `raw.size()` initialized bytes.
        let (record, truncated) = unsafe { decode_body(relationship, raw) };
        if let Some(anomaly) = truncated {
            anomalies.push(anomaly);
        }
        if let Some(record) = record {
            records.push(record);
        }
    }
    anomalies.extend(walk.anomaly());
    (records, anomalies)
}
/// Decode the body a record's `Relationship` field claims it holds.
///
/// Returns the record, plus an anomaly when a trailing array declared more
/// entries than the record could hold. A body that does not fit at all yields
/// `None` rather than a partial record read from a neighbour's bytes.
///
/// # Safety
///
/// `record` must address the bytes of its own declared `Size`, which
/// [`RecordWalk`] establishes before yielding it.
unsafe fn decode_body(
    relationship: LOGICAL_PROCESSOR_RELATIONSHIP,
    record: RawRecord,
) -> (Option<Record>, Option<EnumerationAnomaly>) {
    // windows-sys names these relationship constants in mixed case, not
    // SCREAMING_CASE; that is not this crate's naming to change.
    #[allow(non_upper_case_globals)]
    match relationship {
        RelationProcessorCore
        | RelationProcessorPackage
        | RelationProcessorDie
        | RelationProcessorModule => {
            // SAFETY: forwarded from the caller.
            let (body, anomaly) = unsafe { read_processor_body(record) };
            let wrap = match relationship {
                RelationProcessorCore => Record::ProcessorCore,
                RelationProcessorPackage => Record::ProcessorPackage,
                RelationProcessorDie => Record::ProcessorDie,
                _ => Record::ProcessorModule,
            };
            (body.map(wrap), anomaly)
        }
        RelationCache => {
            // SAFETY: forwarded from the caller.
            let (body, anomaly) = unsafe { read_cache_body(record) };
            (body.map(Record::Cache), anomaly)
        }
        RelationNumaNode | RelationNumaNodeEx => {
            // SAFETY: forwarded from the caller.
            let (body, anomaly) = unsafe { read_numa_body(record) };
            (body.map(Record::NumaNode), anomaly)
        }
        RelationGroup => {
            // SAFETY: forwarded from the caller.
            let (body, anomaly) = unsafe { read_group_body(record) };
            (body.map(Record::Group), anomaly)
        }
        other => (Some(Record::Unknown(other)), None),
    }
}

/// Offset of `field` within a relationship body, from the start of the record.
macro_rules! body {
    ($ty:ty, $field:ident) => {
        UNION_OFFSET + core::mem::offset_of!($ty, $field)
    };
}

/// # Safety
///
/// `record` must address the bytes of its own declared `Size`.
unsafe fn read_processor_body(
    record: RawRecord,
) -> (Option<ProcessorBody>, Option<EnumerationAnomaly>) {
    // SAFETY: forwarded from the caller; every read is bounded by the record.
    let Some((flags, efficiency_class, group_count)) = (unsafe {
        (|| {
            Some((
                record.read::<u8>(body!(PROCESSOR_RELATIONSHIP, Flags))?,
                record.read::<u8>(body!(PROCESSOR_RELATIONSHIP, EfficiencyClass))?,
                record.read::<u16>(body!(PROCESSOR_RELATIONSHIP, GroupCount))?,
            ))
        })()
    }) else {
        return (
            None,
            body_too_short(record, body!(PROCESSOR_RELATIONSHIP, GroupMask)),
        );
    };
    // SAFETY: forwarded from the caller.
    let (group_masks, complete) = unsafe {
        read_group_affinities(
            record,
            body!(PROCESSOR_RELATIONSHIP, GroupMask),
            group_count,
        )
    };
    let anomaly = truncation(record, complete, group_count, group_masks.len());
    (
        Some(ProcessorBody {
            flags,
            efficiency_class,
            group_masks,
        }),
        anomaly,
    )
}

/// A record whose declared `Size` covers the generic header but not the fixed
/// body of the relationship it names.
///
/// Reported rather than dropped: per [D-24](../DESIGN-NOTES.md#d-24) a record
/// that cannot be decoded is an observation, and "too short for the body it
/// claims" is exactly that. `minimum` is the offset at which the relationship's
/// trailing array begins -- i.e. the smallest `Size` that could hold every
/// fixed field the body reader needs.
fn body_too_short(record: RawRecord, minimum: usize) -> Option<EnumerationAnomaly> {
    Some(EnumerationAnomaly::undersized(
        Source::RelationshipWalk,
        record.offset(),
        record.size(),
        minimum,
    ))
}

/// The anomaly for a trailing array that claimed more than the record held.
fn truncation(
    record: RawRecord,
    complete: bool,
    declared: u16,
    decoded: usize,
) -> Option<EnumerationAnomaly> {
    (!complete).then(|| {
        EnumerationAnomaly::truncated_array(
            Source::RelationshipWalk,
            record.offset(),
            usize::from(declared),
            decoded,
        )
    })
}

/// # Safety
///
/// `base` must address a `CACHE_RELATIONSHIP` whose trailing `GroupMask`
/// array (behind its anonymous union) has at least `GroupCount.max(1)`
/// initialized entries -- pre-Windows-20H2 records report `GroupCount == 0`
/// but still have exactly one legacy `GroupMask` entry there (see
/// [`read_legacy_group_affinities`]).
unsafe fn read_cache_body(record: RawRecord) -> (Option<CacheBody>, Option<EnumerationAnomaly>) {
    // SAFETY: forwarded from the caller; every read is bounded by the record.
    let Some((level, associativity, line_size, cache_size, cache_type, group_count)) = (unsafe {
        (|| {
            Some((
                record.read::<u8>(body!(CACHE_RELATIONSHIP, Level))?,
                record.read::<u8>(body!(CACHE_RELATIONSHIP, Associativity))?,
                record.read::<u16>(body!(CACHE_RELATIONSHIP, LineSize))?,
                record.read::<u32>(body!(CACHE_RELATIONSHIP, CacheSize))?,
                record.read::<i32>(body!(CACHE_RELATIONSHIP, Type))?,
                record.read::<u16>(body!(CACHE_RELATIONSHIP, GroupCount))?,
            ))
        })()
    }) else {
        return (
            None,
            body_too_short(record, body!(CACHE_RELATIONSHIP, Anonymous)),
        );
    };
    // SAFETY: forwarded from the caller.
    let (group_masks, complete) = unsafe {
        read_legacy_group_affinities(record, body!(CACHE_RELATIONSHIP, Anonymous), group_count)
    };
    let anomaly = truncation(record, complete, group_count.max(1), group_masks.len());
    (
        Some(CacheBody {
            level,
            associativity,
            line_size,
            cache_size,
            cache_type,
            group_masks,
        }),
        anomaly,
    )
}

/// # Safety
///
/// `base` must address a `NUMA_NODE_RELATIONSHIP` whose trailing `GroupMask`
/// array (behind its anonymous union) has at least `GroupCount.max(1)`
/// initialized entries -- pre-Windows-20H2 records report `GroupCount == 0`
/// but still have exactly one legacy `GroupMask` entry there (see
/// [`read_legacy_group_affinities`]).
unsafe fn read_numa_body(record: RawRecord) -> (Option<NumaNodeBody>, Option<EnumerationAnomaly>) {
    // SAFETY: forwarded from the caller; every read is bounded by the record.
    let Some((node_number, group_count)) = (unsafe {
        (|| {
            Some((
                record.read::<u32>(body!(NUMA_NODE_RELATIONSHIP, NodeNumber))?,
                record.read::<u16>(body!(NUMA_NODE_RELATIONSHIP, GroupCount))?,
            ))
        })()
    }) else {
        return (
            None,
            body_too_short(record, body!(NUMA_NODE_RELATIONSHIP, Anonymous)),
        );
    };
    // SAFETY: forwarded from the caller.
    let (group_masks, complete) = unsafe {
        read_legacy_group_affinities(
            record,
            body!(NUMA_NODE_RELATIONSHIP, Anonymous),
            group_count,
        )
    };
    let anomaly = truncation(record, complete, group_count.max(1), group_masks.len());
    (
        Some(NumaNodeBody {
            node_number,
            group_masks,
        }),
        anomaly,
    )
}

/// # Safety
///
/// `record` must address the bytes of its own declared `Size`.
unsafe fn read_group_body(record: RawRecord) -> (Option<GroupBody>, Option<EnumerationAnomaly>) {
    // SAFETY: forwarded from the caller; the read is bounded by the record.
    let Some(active_group_count) =
        (unsafe { record.read::<u16>(body!(GROUP_RELATIONSHIP, ActiveGroupCount)) })
    else {
        return (
            None,
            body_too_short(record, body!(GROUP_RELATIONSHIP, GroupInfo)),
        );
    };
    // SAFETY: forwarded from the caller; bounded by the record, so an
    // `ActiveGroupCount` larger than the record can hold yields only what fits.
    let (raw, complete) = unsafe {
        record.read_array::<PROCESSOR_GROUP_INFO>(
            body!(GROUP_RELATIONSHIP, GroupInfo),
            usize::from(active_group_count),
        )
    };
    let group_info: Vec<_> = raw
        .into_iter()
        .map(|entry| GroupInfo {
            maximum_processor_count: entry.MaximumProcessorCount,
            active_processor_count: entry.ActiveProcessorCount,
            active_processor_mask: entry.ActiveProcessorMask,
        })
        .collect();
    let anomaly = truncation(record, complete, active_group_count, group_info.len());
    (Some(GroupBody { group_info }), anomaly)
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
