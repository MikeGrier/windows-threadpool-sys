// Copyright (c) Mike Grier.
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::mem::zeroed;

use serde::Serialize;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::ProcessStatus::{
    PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx,
};
use windows_sys::Win32::System::SystemInformation::{GROUP_AFFINITY, GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessorNumberEx, GetCurrentThread, GetThreadTimes,
    SetThreadGroupAffinity,
};
use windows_topology_sys::{MachineMemoryTopology, Observed, ProcessorId};

use crate::{failed, invalid};

pub(crate) fn select_processors(
    machine: &MachineMemoryTopology,
    requested: Option<[ProcessorId; 2]>,
) -> io::Result<[ProcessorId; 2]> {
    if let Some(pair) = requested {
        if pair[0] == pair[1] {
            return Err(invalid("the experiment requires two distinct processors"));
        }
        for id in pair {
            if id.number as u32 >= usize::BITS
                || !machine.processors.iter().any(|p| p.id == id && p.online)
            {
                return Err(invalid(format!(
                    "processor {id:?} is not an active representable processor"
                )));
            }
        }
        return Ok(pair);
    }
    let facts = machine.shard_set();
    for (i, first) in facts.iter().enumerate().filter(|(_, p)| p.online) {
        let Observed::Known(node) = first.memory_domain else {
            continue;
        };
        let Some(first_core) = first.core else {
            continue;
        };
        if let Some(second) = facts[i + 1..].iter().find(|p| {
            p.online
                && p.core.is_some_and(|core| core != first_core)
                && p.efficiency_class == first.efficiency_class
                && matches!(p.memory_domain, Observed::Known(other) if other == node)
        }) {
            return Ok([first.id, second.id]);
        }
    }
    Err(failed(
        "no distinct-core, same-class pair in one observed memory domain; supply an explicit processor pair",
    ))
}

pub(crate) fn pin(id: ProcessorId) -> io::Result<()> {
    let mask = 1_usize
        .checked_shl(id.number.into())
        .ok_or_else(|| invalid("invalid processor"))?;
    let affinity = GROUP_AFFINITY {
        Mask: mask,
        Group: id.group,
        Reserved: [0; 3],
    };
    // SAFETY: both pseudo-handle and initialized affinity refer to the calling worker.
    if unsafe { SetThreadGroupAffinity(GetCurrentThread(), &affinity, std::ptr::null_mut()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if current_processor() != id {
        return Err(failed("achieved processor differs from requested binding"));
    }
    Ok(())
}

pub(crate) fn current_processor() -> ProcessorId {
    // SAFETY: initialized output storage is passed to the current-processor query.
    let mut value = unsafe { zeroed() };
    unsafe { GetCurrentProcessorNumberEx(&mut value) };
    ProcessorId {
        group: value.Group,
        number: value.Number,
    }
}

pub(crate) fn cpu_ns() -> io::Result<u64> {
    // SAFETY: FILETIME is a plain integer structure and all output pointers are valid.
    let mut created: FILETIME = unsafe { zeroed() };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let ticks = |v: FILETIME| (u64::from(v.dwHighDateTime) << 32) | u64::from(v.dwLowDateTime);
    Ok((ticks(kernel) + ticks(user)) * 100)
}

/// SDK working-set bitfield identities. Changing these changes the decoder contract.
mod working_set {
    pub const VALID: usize = 1;
    pub const NODE_SHIFT: u32 = 16;
    pub const NODE_MASK: usize = (1 << 6) - 1;
}

#[derive(Debug, Serialize)]
pub struct Residency {
    pub pages_by_node: BTreeMap<usize, usize>,
    pub unresident_pages: usize,
}

pub(crate) fn resident_node(flags: usize) -> Option<usize> {
    (flags & working_set::VALID != 0)
        .then_some((flags >> working_set::NODE_SHIFT) & working_set::NODE_MASK)
}

pub(crate) fn residency(buffers: &[Vec<u8>]) -> io::Result<Residency> {
    // SAFETY: SYSTEM_INFO is output-only plain storage.
    let mut system: SYSTEM_INFO = unsafe { zeroed() };
    unsafe { GetSystemInfo(&mut system) };
    let page_size = system.dwPageSize as usize;
    if !page_size.is_power_of_two() {
        return Err(failed("system page size is not a power of two"));
    }
    let mut pages = BTreeSet::new();
    for buffer in buffers {
        let start = buffer.as_ptr() as usize;
        let first_page = start & !(page_size - 1);
        let end = start
            .checked_add(buffer.len())
            .ok_or_else(|| failed("address overflow"))?;
        pages.extend((first_page..end).step_by(page_size));
    }
    let mut result = Residency {
        pages_by_node: BTreeMap::new(),
        unresident_pages: 0,
    };
    for address in pages {
        let mut info = PSAPI_WORKING_SET_EX_INFORMATION {
            VirtualAddress: address as *mut _,
            // SAFETY: the union is output-only storage for this query.
            VirtualAttributes: unsafe { zeroed() },
        };
        // SAFETY: info is a live correctly-sized single entry; all queried buffers remain owned.
        if unsafe {
            QueryWorkingSetEx(
                GetCurrentProcess(),
                (&mut info as *mut PSAPI_WORKING_SET_EX_INFORMATION).cast(),
                size_of::<PSAPI_WORKING_SET_EX_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: Flags is the integer view of the filled SDK union.
        match resident_node(unsafe { info.VirtualAttributes.Flags }) {
            Some(node) => *result.pages_by_node.entry(node).or_default() += 1,
            None => result.unresident_pages += 1,
        }
    }
    Ok(result)
}
