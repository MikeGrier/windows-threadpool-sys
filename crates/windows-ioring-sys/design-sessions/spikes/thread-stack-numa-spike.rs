// Copyright (c) Mike Grier
//! Spike: does creation-time affinity govern where a thread's *stack* lives?
//!
//! **NOT YET RUN ON HARDWARE THAT CAN ANSWER IT.** Checked in as a ready
//! instrument, not a result. It needs a machine with at least two NUMA nodes
//! that both have processors. On a single-node machine every answer is `0` and
//! the run proves only that the apparatus works.
//!
//! # Why this matters
//!
//! A thread's stack is allocated when the thread is created, on whatever node
//! the *creating* thread's policy selects. If that is true and irreversible,
//! then binding a thread's affinity after it starts cannot move its stack --
//! every local, every spill, every call frame stays remote for the life of the
//! thread. That is the entire argument for a domain runtime *constructing* its
//! threads with `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY` already set, rather than
//! spawning them and calling `SetThreadGroupAffinity` afterwards.
//!
//! The argument is currently **assumed**. This measures it.
//!
//! # Design
//!
//! Three threads, which together discriminate the possibilities:
//!
//!   A `created-far`  -- created with `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY`
//!                       naming the far node.
//!   B `control-near` -- created with no attribute list at all. Its stack
//!                       should sit on the creator's node, and it is the
//!                       baseline the other two are read against.
//!   C `bound-after`  -- created with no attributes, then immediately
//!                       `SetThreadGroupAffinity` to the far node from inside
//!                       the thread. This is the shape a naive consumer writes.
//!
//! Each thread reports the NUMA node of **two** stack pages, because Windows
//! commits stack pages on demand and the two may be placed by different
//! mechanisms:
//!
//!   shallow -- a local in the entry frame, on a page committed at or near
//!              thread creation;
//!   deep    -- a local behind a 64 KiB frame, on a page committed later, while
//!              the thread is already running under its final affinity.
//!
//! Reading the two together is what makes the result interpretable:
//!
//! | A.shallow | C.shallow | Conclusion |
//! |---|---|---|
//! | far | near | Creation-time affinity governs stack placement. The builder is justified and binding afterwards genuinely cannot fix it. |
//! | near | near | Creation-time affinity does **not** govern it. The builder's principal justification fails and the design must stop claiming it. |
//! | far | far | Something moved C's stack too; investigate before believing either. |
//!
//! And independently, for either thread: `shallow != deep` means pages are
//! placed by **first touch** under the running affinity, not by a decision made
//! once at creation -- which would mean a deep stack is local even when the
//! shallow one is not, and that the whole question is subtler than the design
//! assumes.
//!
//! `Valid` is reported alongside every node, because `QueryWorkingSetEx` only
//! fills `Node` for a resident page. A node read from a non-resident page is
//! meaningless, and treating one as an answer is the obvious way to get a
//! confident wrong result here.
//!
//! Run with:
//! ```toml
//! [dependencies]
//! windows-sys = { version = "0.61.2", default-features = false, features = [
//!     "Win32_Foundation", "Win32_Security", "Win32_System_Threading",
//!     "Win32_System_SystemInformation", "Win32_System_ProcessStatus",
//! ] }
//! ```

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::ProcessStatus::{
    PSAPI_WORKING_SET_EX_INFORMATION, QueryWorkingSetEx,
};
use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
use windows_sys::Win32::System::Threading::{
    CreateRemoteThreadEx, DeleteProcThreadAttributeList, GetCurrentProcess, GetCurrentThread,
    GetNumaHighestNodeNumber, GetNumaNodeProcessorMaskEx, INFINITE,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY,
    SetThreadGroupAffinity, UpdateProcThreadAttribute, WaitForSingleObject,
};

/// Bit layout of `PSAPI_WORKING_SET_EX_BLOCK` for a valid page: `Valid` at bit
/// 0, `ShareCount` at 1..4, `Win32Protection` at 4..15, `Shared` at 15, and
/// `Node` at 16..22. Extracted from `Flags` rather than through the anonymous
/// bitfield struct, so this does not depend on how the binding chose to model
/// it.
mod ws_block {
    pub const VALID: usize = 1;
    pub const NODE_SHIFT: u32 = 16;
    pub const NODE_MASK: usize = 0x3F;
}

/// The NUMA node of the page containing `addr`, and whether that page was
/// resident. `None` means the query itself failed.
fn page_node(addr: *const c_void) -> Option<(bool, u32)> {
    let mut info = PSAPI_WORKING_SET_EX_INFORMATION {
        VirtualAddress: addr as *mut c_void,
        ..unsafe { std::mem::zeroed() }
    };
    let ok = unsafe {
        QueryWorkingSetEx(
            GetCurrentProcess(),
            (&raw mut info).cast::<c_void>(),
            u32::try_from(size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()).unwrap(),
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: reading the `Flags` arm of the union, which is always valid to
    // read as a `usize` regardless of which arm was written.
    let flags = unsafe { info.VirtualAttributes.Flags };
    let valid = flags & ws_block::VALID != 0;
    let node = ((flags >> ws_block::NODE_SHIFT) & ws_block::NODE_MASK) as u32;
    Some((valid, node))
}

#[derive(Default, Clone, Copy)]
struct Probe {
    valid: bool,
    node: u32,
    queried: bool,
}

impl Probe {
    fn take(addr: *const c_void) -> Self {
        match page_node(addr) {
            Some((valid, node)) => Probe {
                valid,
                node,
                queried: true,
            },
            None => Probe::default(),
        }
    }

    fn show(self) -> String {
        if !self.queried {
            "query FAILED".to_string()
        } else if !self.valid {
            "page not resident -- node meaningless".to_string()
        } else {
            format!("node {}", self.node)
        }
    }
}

#[repr(C)]
struct Slot {
    label: &'static str,
    /// Thread C: bind to `far` from inside the thread, after it is running.
    bind_far_after_start: bool,
    far: GROUP_AFFINITY,
    shallow: Probe,
    deep: Probe,
    bind_after_ok: Option<bool>,
}

/// Forces a page deeper in the stack to be committed, then probes it. The
/// array is written to, because a page that is merely reserved is not resident
/// and its reported node would be meaningless.
#[inline(never)]
fn deep_probe() -> Probe {
    let mut filler = [0_u8; 64 * 1024];
    // Touch both ends so the whole span is committed, and defeat any attempt to
    // optimize the array away.
    filler[0] = 1;
    let last = filler.len() - 1;
    filler[last] = 1;
    std::hint::black_box(&filler);
    Probe::take((&raw const filler[last]).cast::<c_void>())
}

unsafe extern "system" fn entry(param: *mut c_void) -> u32 {
    // SAFETY: `param` is the `Slot` this thread was created for; `main` joins
    // every thread before the slots go out of scope.
    let slot = unsafe { &mut *param.cast::<Slot>() };

    if slot.bind_far_after_start {
        let ok = unsafe { SetThreadGroupAffinity(GetCurrentThread(), &raw const slot.far, std::ptr::null_mut()) };
        slot.bind_after_ok = Some(ok != 0);
    }

    let shallow_local = 0_u64;
    std::hint::black_box(&shallow_local);
    slot.shallow = Probe::take((&raw const shallow_local).cast::<c_void>());
    slot.deep = deep_probe();
    0
}

/// Creates a thread, optionally with a group-affinity attribute applied at
/// creation. Returns the thread handle.
fn spawn(slot: &mut Slot, affinity: Option<&GROUP_AFFINITY>) -> Result<HANDLE, String> {
    let param = (&raw mut *slot).cast::<c_void>();

    let Some(affinity) = affinity else {
        // Control: no attribute list at all, which is what every ordinary
        // spawn does, including `std::thread`.
        let h = unsafe {
            CreateRemoteThreadEx(
                GetCurrentProcess(),
                std::ptr::null(),
                0,
                Some(entry),
                param,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        return if h.is_null() {
            Err(format!("CreateRemoteThreadEx: {}", std::io::Error::last_os_error()))
        } else {
            Ok(h)
        };
    };

    // Two-pass sizing: the first call is expected to fail with
    // ERROR_INSUFFICIENT_BUFFER and fill in `size`.
    let mut size: usize = 0;
    unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &raw mut size) };
    if size == 0 {
        return Err("InitializeProcThreadAttributeList reported a zero size".into());
    }
    let mut buf = vec![0_u8; size];
    let list = buf.as_mut_ptr().cast::<c_void>();
    if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &raw mut size) } == 0 {
        return Err(format!(
            "InitializeProcThreadAttributeList: {}",
            std::io::Error::last_os_error()
        ));
    }

    // `affinity` must outlive the CreateRemoteThreadEx call: the attribute list
    // stores the pointer, not a copy.
    let ok = unsafe {
        UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY as usize,
            (affinity as *const GROUP_AFFINITY).cast::<c_void>(),
            size_of::<GROUP_AFFINITY>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        let e = std::io::Error::last_os_error();
        unsafe { DeleteProcThreadAttributeList(list) };
        return Err(format!("UpdateProcThreadAttribute: {e}"));
    }

    let handle = unsafe {
        CreateRemoteThreadEx(
            GetCurrentProcess(),
            std::ptr::null(),
            0,
            Some(entry),
            param,
            0,
            list,
            std::ptr::null_mut(),
        )
    };
    unsafe { DeleteProcThreadAttributeList(list) };

    if handle.is_null() {
        Err(format!(
            "CreateRemoteThreadEx (with affinity): {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(handle)
    }
}

fn join(handle: HANDLE) {
    unsafe {
        if WaitForSingleObject(handle, INFINITE) != WAIT_OBJECT_0 {
            eprintln!("WaitForSingleObject: {}", std::io::Error::last_os_error());
        }
        CloseHandle(handle);
    }
}

fn main() {
    let mut highest: u32 = 0;
    if unsafe { GetNumaHighestNodeNumber(&raw mut highest) } == 0 {
        eprintln!(
            "GetNumaHighestNodeNumber failed: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    println!("highest NUMA node number: {highest}  (nodes: {})", highest + 1);

    if highest == 0 {
        println!();
        println!("*** VACUOUS ON THIS MACHINE ***");
        println!("One NUMA node means every answer below is 0 and nothing is");
        println!("discriminated. Running anyway only validates the apparatus.");
    }

    // The far node is the highest-numbered one that actually has processors; a
    // memory-only node cannot host a thread, so it cannot answer this question.
    let mut far = GROUP_AFFINITY::default();
    let mut far_node = u32::MAX;
    for candidate in (0..=highest).rev() {
        let mut ga = GROUP_AFFINITY::default();
        if unsafe { GetNumaNodeProcessorMaskEx(candidate as u16, &raw mut ga) } != 0 && ga.Mask != 0
        {
            far = ga;
            far_node = candidate;
            break;
        }
    }
    if far_node == u32::MAX {
        println!("no NUMA node reports any processors; cannot proceed.");
        return;
    }
    println!(
        "far node chosen: {far_node} (group {}, mask {:#x})",
        far.Group, far.Mask
    );

    let mut slots = [
        Slot {
            label: "A created-far  (affinity attribute at creation)",
            bind_far_after_start: false,
            far,
            shallow: Probe::default(),
            deep: Probe::default(),
            bind_after_ok: None,
        },
        Slot {
            label: "B control-near (no attribute list at all)",
            bind_far_after_start: false,
            far,
            shallow: Probe::default(),
            deep: Probe::default(),
            bind_after_ok: None,
        },
        Slot {
            label: "C bound-after  (spawned plain, bound to far)",
            bind_far_after_start: true,
            far,
            shallow: Probe::default(),
            deep: Probe::default(),
            bind_after_ok: None,
        },
    ];

    let with_affinity = [true, false, false];
    let mut handles = Vec::new();
    for (slot, &use_attr) in slots.iter_mut().zip(with_affinity.iter()) {
        match spawn(slot, if use_attr { Some(&far) } else { None }) {
            Ok(h) => handles.push(h),
            Err(e) => {
                println!("spawn failed for {}: {e}", slot.label);
                return;
            }
        }
    }
    for h in handles {
        join(h);
    }

    println!("\n{:<50} {:<28} {}", "thread", "shallow stack page", "deep stack page");
    for slot in &slots {
        println!(
            "{:<50} {:<28} {}",
            slot.label,
            slot.shallow.show(),
            slot.deep.show()
        );
        if let Some(ok) = slot.bind_after_ok {
            println!("{:<50} SetThreadGroupAffinity ok = {ok}", "");
        }
    }

    // Only interpret when the pages are resident and the machine has more than
    // one node; otherwise say so rather than printing a confident conclusion.
    let usable = slots
        .iter()
        .all(|s| s.shallow.queried && s.shallow.valid && s.deep.queried && s.deep.valid);
    println!();
    if highest == 0 {
        println!("=> VACUOUS: one node. Apparatus works; question unanswered.");
    } else if !usable {
        println!("=> INCONCLUSIVE: a probed page was not resident or the query failed.");
    } else {
        let (a, b, c) = (
            slots[0].shallow.node,
            slots[1].shallow.node,
            slots[2].shallow.node,
        );
        if a == far_node && c == b {
            println!("=> Creation-time affinity GOVERNS stack placement, and binding");
            println!("   afterwards does not move the stack. The thread builder is justified.");
        } else if a == b && c == b {
            println!("=> Creation-time affinity does NOT govern stack placement.");
            println!("   The builder's principal justification FAILS; stop claiming it.");
        } else {
            println!("=> Unexpected combination (A={a}, B={b}, C={c}, far={far_node}).");
            println!("   Investigate before believing any of it.");
        }
        for slot in &slots {
            if slot.shallow.node != slot.deep.node {
                println!(
                    "   NOTE: {} has shallow != deep, so pages are placed by FIRST TOUCH",
                    slot.label.split_whitespace().next().unwrap_or("?")
                );
                println!("   under the running affinity, not once at creation.");
            }
        }
    }
}
