// Copyright (c) 2026 Mike Grier
//! The safe `GetSystemCpuSetInformation` walk.
//!
//! A second, independent view of the same processors [`crate::walk`] describes,
//! read from a different kernel path. Windows exposes two processor-topology
//! APIs and they are not the same API twice: this one reports **availability**
//! (parked, and whether the processor is allocated to this process at all),
//! Windows's **own** last-level-cache grouping, and a scheduling class and
//! allocation tag that the relationship walk has no equivalent for.
//!
//! The buffer discipline matches `GetLogicalProcessorInformationEx`'s, and for
//! the same reasons:
//!
//! - Size first with a null buffer, which fails with `ERROR_INSUFFICIENT_BUFFER`
//!   and reports the byte count.
//! - Records are **variable length**: advance by each record's own `Size` field,
//!   never by `size_of::<SYSTEM_CPU_SET_INFORMATION>()`. The struct's declared
//!   size describes today's `CpuSetInformation` record, and a future type may be
//!   longer.
//! - Read every field with `read_unaligned`, since a record's start is only as
//!   aligned as the running `Size` sum makes it.
//!
//! # What this deliberately does not do
//!
//! It does not reconcile anything. `CoreIndex`, `NumaNodeIndex` and
//! `EfficiencyClass` duplicate facts the relationship walk already reports, and
//! the two paths can disagree -- under a hypervisor, or where one is stale.
//! Merging them here would silently pick a winner and destroy the disagreement,
//! which is the one thing a second observer is *for*. The records come back as
//! what they are; deciding what to do when they differ is tracked separately.

use std::io;

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};

/// One processor as the CPU-set API describes it.
///
/// Field-for-field what the record carries, with the two bitfield unions
/// decoded. Nothing is interpreted and nothing is cross-checked against the
/// relationship walk -- see this module's note on why reconciling here would
/// destroy the only thing a second observer is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CpuSet {
    /// The CPU set's id, which is what `SetThreadSelectedCpuSets` takes. Not a
    /// processor number, and not interchangeable with one.
    pub id: u32,
    /// The processor group.
    pub group: u16,
    /// The processor's number within its group.
    pub logical_processor_index: u8,
    /// Windows's index for the owning core.
    pub core_index: u8,
    /// **Windows's own** last-level-cache grouping: processors sharing a value
    /// share an LLC in the scheduler's view. Deliberately kept even though the
    /// relationship walk also reports caches, because this is the OS's opinion
    /// rather than a partition derived from firmware records, and the two are
    /// answers to different questions.
    pub last_level_cache_index: u8,
    /// Windows's index for the owning NUMA node.
    pub numa_node_index: u8,
    /// The scheduler's efficiency class. Carried as reported, with no sentinel:
    /// a processor absent from this enumeration has no record at all rather
    /// than a record holding a stand-in value. Contrast
    /// [`Processor::capacity`](crate::Processor::capacity), which uses `0` for
    /// both "class zero" and "not known".
    pub efficiency_class: u8,
    /// The processor is parked, so the scheduler is currently avoiding it.
    pub parked: bool,
    /// The processor is allocated.
    pub allocated: bool,
    /// The processor is allocated **to this process**. A planner that ignores
    /// this places work on processors the process may not use, which is a wrong
    /// plan rather than a slow one.
    pub allocated_to_target_process: bool,
    /// The processor is marked real-time.
    pub real_time: bool,
    /// The scheduling class, which shares its union with a reserved `u32`, so
    /// confirm its meaning against current SDK documentation before relying on
    /// it rather than treating this field as self-describing.
    pub scheduling_class: u8,
    /// The allocation tag.
    pub allocation_tag: u64,
}

/// Bit positions within `AllFlags`, named rather than written inline.
///
/// Changing any value is a breaking change: these mirror the SDK's bitfield
/// order, which is part of the ABI rather than this crate's choice.
mod flags {
    pub(super) const PARKED: u8 = 1 << 0;
    pub(super) const ALLOCATED: u8 = 1 << 1;
    pub(super) const ALLOCATED_TO_TARGET_PROCESS: u8 = 1 << 2;
    pub(super) const REAL_TIME: u8 = 1 << 3;
}

/// Enumerate the CPU sets the current process can see.
///
/// Passing a null process handle asks about the calling process, so
/// `allocated_to_target_process` answers "may *we* use it" rather than "does it
/// exist".
///
/// # Errors
///
/// Returns any error from `GetSystemCpuSetInformation` other than the expected
/// sizing failure.
pub(crate) fn enumerate() -> io::Result<Vec<CpuSet>> {
    let mut length: u32 = 0;
    // SAFETY: a null buffer with a zero length and a valid out-pointer, which is
    // the documented sizing call. A null process handle names this process.
    let probe = unsafe {
        GetSystemCpuSetInformation(
            std::ptr::null_mut(),
            0,
            &raw mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if probe != 0 {
        // Succeeding on the sizing call would mean zero bytes were needed, so
        // there is nothing to report.
        return Ok(Vec::new());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(error);
    }
    if length == 0 {
        return Ok(Vec::new());
    }

    // `u64`-backed storage for the same reason the relationship walk uses it:
    // it guarantees 8-byte alignment for every record header regardless of what
    // a `Vec<u8>` allocation would have happened to provide. `AllocationTag` is
    // 8-byte-sized, so this is not merely tidiness.
    let mut storage = vec![0_u64; (length as usize).div_ceil(8)];
    let buffer = storage.as_mut_ptr().cast::<u8>();
    let mut actual_length = length;
    // SAFETY: `buffer` points to `storage`, whose byte length is at least
    // `length` (the size the probe just reported) and is 8-byte aligned;
    // `actual_length` is a valid in/out length pointer.
    let ok = unsafe {
        GetSystemCpuSetInformation(
            buffer.cast(),
            length,
            &raw mut actual_length,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `buffer` holds `actual_length` bytes written by the call above:
    // consecutive `SYSTEM_CPU_SET_INFORMATION` records whose `Size` fields sum
    // to `actual_length`, per the API's contract.
    Ok(unsafe { decode(buffer.cast_const(), actual_length) })
}

const SIZE_OFFSET: usize = core::mem::offset_of!(SYSTEM_CPU_SET_INFORMATION, Size);
const TYPE_OFFSET: usize = core::mem::offset_of!(SYSTEM_CPU_SET_INFORMATION, Type);
const UNION_OFFSET: usize = core::mem::offset_of!(SYSTEM_CPU_SET_INFORMATION, Anonymous);

/// Read a `T` from `base + offset` without assuming alignment.
///
/// # Safety
///
/// `base + offset` must address at least `size_of::<T>()` initialized bytes.
unsafe fn read_at<T: Copy>(base: *const u8, offset: usize) -> T {
    // SAFETY: forwarded from the caller.
    unsafe { base.add(offset).cast::<T>().read_unaligned() }
}

/// Walk `length` bytes of consecutive records.
///
/// # Safety
///
/// `base` must address `length` initialized bytes laid out as consecutive
/// `SYSTEM_CPU_SET_INFORMATION` records.
unsafe fn decode(base: *const u8, length: u32) -> Vec<CpuSet> {
    // Offsets within the `CpuSet` arm of the record's union, computed from the
    // generated types so a binding change moves them rather than silently
    // shifting what is read.
    use windows_sys::Win32::System::SystemInformation::{
        SYSTEM_CPU_SET_INFORMATION_0, SYSTEM_CPU_SET_INFORMATION_0_0,
    };
    const CPUSET_OFFSET: usize = core::mem::offset_of!(SYSTEM_CPU_SET_INFORMATION_0, CpuSet);
    macro_rules! field {
        ($name:ident) => {
            UNION_OFFSET
                + CPUSET_OFFSET
                + core::mem::offset_of!(SYSTEM_CPU_SET_INFORMATION_0_0, $name)
        };
    }

    let mut records = Vec::new();
    let mut offset = 0_usize;
    let length = length as usize;

    while offset + SIZE_OFFSET + size_of::<u32>() <= length {
        let record = unsafe { base.add(offset) };
        // SAFETY: the bound above proved `Size` itself is in range.
        let size = unsafe { read_at::<u32>(record, SIZE_OFFSET) } as usize;
        // A zero or oversized `Size` would loop forever or read past the end.
        // Windows does not produce either, and trusting it anyway is how a
        // hostile or corrupt buffer becomes a hang instead of a stop.
        if size == 0 || offset + size > length {
            break;
        }

        // SAFETY: `size` bytes from `record` are in range, and this record is at
        // least a full `SYSTEM_CPU_SET_INFORMATION`, so every field below is
        // within it.
        let kind = unsafe { read_at::<i32>(record, TYPE_OFFSET) };
        if kind == CpuSetInformation && size >= size_of::<SYSTEM_CPU_SET_INFORMATION>() {
            // SAFETY: as above; each offset is computed from the generated type.
            let all_flags = unsafe { read_at::<u8>(record, field!(Anonymous1)) };
            records.push(CpuSet {
                id: unsafe { read_at::<u32>(record, field!(Id)) },
                group: unsafe { read_at::<u16>(record, field!(Group)) },
                logical_processor_index: unsafe {
                    read_at::<u8>(record, field!(LogicalProcessorIndex))
                },
                core_index: unsafe { read_at::<u8>(record, field!(CoreIndex)) },
                last_level_cache_index: unsafe {
                    read_at::<u8>(record, field!(LastLevelCacheIndex))
                },
                numa_node_index: unsafe { read_at::<u8>(record, field!(NumaNodeIndex)) },
                efficiency_class: unsafe { read_at::<u8>(record, field!(EfficiencyClass)) },
                parked: all_flags & flags::PARKED != 0,
                allocated: all_flags & flags::ALLOCATED != 0,
                allocated_to_target_process: all_flags & flags::ALLOCATED_TO_TARGET_PROCESS != 0,
                real_time: all_flags & flags::REAL_TIME != 0,
                scheduling_class: unsafe { read_at::<u8>(record, field!(Anonymous2)) },
                allocation_tag: unsafe { read_at::<u64>(record, field!(AllocationTag)) },
            });
        }

        offset += size;
    }

    records
}

#[cfg(test)]
mod tests;
