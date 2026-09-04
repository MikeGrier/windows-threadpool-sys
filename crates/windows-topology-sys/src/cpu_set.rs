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
use crate::records::RecordWalk;
use std::io;

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows_sys::Win32::System::SystemInformation::{
    CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

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
    ///
    /// **Measured to carry no information on Windows 11 25H2** -- see
    /// [`Self::allocated_to_target_process`] and D-23 in `DESIGN-NOTES.md`.
    pub parked: bool,
    /// The processor is allocated to some process through the CPU-set API.
    ///
    /// **Measured to carry no information on Windows 11 25H2** -- see
    /// [`Self::allocated_to_target_process`].
    pub allocated: bool,
    /// The processor is allocated **to this process** through the CPU-set API.
    ///
    /// # This is not "may we run here", and it is not populated
    ///
    /// Two corrections, both established by experiment rather than reasoning
    /// (D-23 in `DESIGN-NOTES.md`).
    ///
    /// It does not mean the process may use the processor. It means the CPU set
    /// was explicitly allocated through `SetProcessDefaultCpuSets` or
    /// `SetThreadSelectedCpuSets`, which a process that never called them has
    /// not done -- so `false` is the ordinary answer for an ordinary process on
    /// every processor it is perfectly free to run on.
    ///
    /// And on Windows 11 25H2 (10.0.26200.9168, AMD64) it is not populated at
    /// all. Calling `SetProcessDefaultCpuSets` successfully, and confirming
    /// with `GetProcessDefaultCpuSets` that the allocation stuck, still leaves
    /// the whole `AllFlags` byte reading `0x00` for every processor -- under a
    /// null handle, the current-process pseudo-handle, and a real `OpenProcess`
    /// handle alike.
    ///
    /// **So do not branch on this.** A reader of `false` is reading a byte the
    /// kernel did not write, not a fact about the machine.
    pub allocated_to_target_process: bool,
    /// The processor is marked real-time.
    ///
    /// **Measured to carry no information on Windows 11 25H2** -- see
    /// [`Self::allocated_to_target_process`].
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
///
/// # These are unverified, and cannot be verified from this machine
///
/// `AllFlags` reads `0x00` for every processor on the build measured (D-23),
/// even after allocating CPU sets to this process -- so no bit has ever been
/// observed set, and nothing here distinguishes a correct transcription from a
/// wrong one. They stand on the SDK's declared order alone.
///
/// That is survivable only because nothing branches on them. If a consumer ever
/// does, these need a machine that populates the byte first.
mod flags {
    pub(super) const PARKED: u8 = 1 << 0;
    pub(super) const ALLOCATED: u8 = 1 << 1;
    pub(super) const ALLOCATED_TO_TARGET_PROCESS: u8 = 1 << 2;
    pub(super) const REAL_TIME: u8 = 1 << 3;
}

/// Enumerate the CPU sets the current process can see.
///
/// The current process is named explicitly, because `Process` is **not**
/// optional in the way a null handle suggests: Microsoft documents it as the
/// process used to compute `AllocatedToTargetProcess`, so passing null means no
/// allocation check is made rather than "ask about me". Raised in PR #56
/// review, where an earlier comment here claimed the opposite.
///
/// It makes no difference to the flags on the build this was measured against,
/// which read zero under a null handle, the pseudo-handle, and a real
/// `OpenProcess` handle alike -- see [`CpuSet::allocated_to_target_process`] and
/// D-23 in `DESIGN-NOTES.md`. Asking the documented question anyway is what
/// makes the zero a fact about the build rather than about the call.
///
/// # Errors
///
/// Returns any error from `GetSystemCpuSetInformation` other than the expected
/// sizing failure.
pub(crate) fn enumerate() -> io::Result<(Vec<CpuSet>, Option<EnumerationAnomaly>)> {
    let mut length: u32 = 0;
    // SAFETY: a null buffer with a zero length and a valid out-pointer, which is
    // the documented sizing call. `GetCurrentProcess` is a pseudo-handle needing
    // no close, and is the documented way to ask about this process.
    let probe = unsafe {
        GetSystemCpuSetInformation(
            std::ptr::null_mut(),
            0,
            &raw mut length,
            GetCurrentProcess(),
            0,
        )
    };
    if probe != 0 {
        // Succeeding on the sizing call would mean zero bytes were needed, so
        // there is nothing to report.
        return Ok((Vec::new(), None));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
        return Err(error);
    }
    if length == 0 {
        return Ok((Vec::new(), None));
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
            GetCurrentProcess(),
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

/// Walk `length` bytes of consecutive records.
///
/// # Safety
///
/// `base` must address `length` initialized bytes laid out as consecutive
/// `SYSTEM_CPU_SET_INFORMATION` records.
unsafe fn decode(base: *const u8, length: u32) -> (Vec<CpuSet>, Option<EnumerationAnomaly>) {
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

    // SAFETY: forwarded from this function's own contract.
    let mut walk = unsafe {
        RecordWalk::new(
            base,
            length,
            SIZE_OFFSET,
            size_of::<SYSTEM_CPU_SET_INFORMATION>(),
            Source::CpuSets,
        )
    };

    let mut records = Vec::new();
    for record in &mut walk {
        // Every read below is bounded by the record's own `Size`, and the walk
        // has already established that `Size` covers a full
        // `SYSTEM_CPU_SET_INFORMATION` -- so none of these can fail. They are
        // written as options rather than asserted because the alternative to a
        // `None` that skips a record is a panic, and per D-24 this crate does
        // not panic over the shape of someone else's buffer.
        // SAFETY: the walk yielded a record addressing `size` initialized bytes.
        let decoded = unsafe {
            (|| {
                if record.read::<i32>(TYPE_OFFSET)? != CpuSetInformation {
                    return None;
                }
                let all_flags = record.read::<u8>(field!(Anonymous1))?;
                Some(CpuSet {
                    id: record.read(field!(Id))?,
                    group: record.read(field!(Group))?,
                    logical_processor_index: record.read(field!(LogicalProcessorIndex))?,
                    core_index: record.read(field!(CoreIndex))?,
                    last_level_cache_index: record.read(field!(LastLevelCacheIndex))?,
                    numa_node_index: record.read(field!(NumaNodeIndex))?,
                    efficiency_class: record.read(field!(EfficiencyClass))?,
                    parked: all_flags & flags::PARKED != 0,
                    allocated: all_flags & flags::ALLOCATED != 0,
                    allocated_to_target_process: all_flags & flags::ALLOCATED_TO_TARGET_PROCESS
                        != 0,
                    real_time: all_flags & flags::REAL_TIME != 0,
                    scheduling_class: record.read(field!(Anonymous2))?,
                    allocation_tag: record.read(field!(AllocationTag))?,
                })
            })()
        };
        if let Some(cpu_set) = decoded {
            records.push(cpu_set);
        }
    }

    (records, walk.anomaly())
}

#[cfg(test)]
mod tests;
