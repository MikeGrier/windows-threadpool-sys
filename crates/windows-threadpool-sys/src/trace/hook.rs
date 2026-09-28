// Copyright (c) Mike Grier.

//! Inline hooks on `ntdll`'s worker-factory syscall stubs.
//!
//! This exists for one investigation: a default-pool stall in which the pool
//! holds parked workers and a queued completion packet and does not put the
//! two together (see
//! [the ioring crate's STALL-TIMELINE.md](../../../windows-ioring-sys/STALL-TIMELINE.md)).
//! Everything above the factory has been measured from outside and is
//! consistent; what is left is what the factory itself was told and what it
//! believes. Both are reachable only by watching the syscalls that carry them.
//!
//! # Why this can be done without a disassembler
//!
//! A general inline hooker has to decode the instructions it displaces, so it
//! can relocate them into a trampoline. That is where most of the difficulty
//! and most of the bugs in this technique live.
//!
//! None of it is needed here, because every `ntdll` `Nt*` entry point is the
//! *same* stub:
//!
//! ```text
//! 4C 8B D1                        mov r10, rcx
//! B8 <ssn:u32>                    mov eax, <system call number>
//! F6 04 25 08 03 FE 7F 01         test byte ptr [7FFE0308h], 1
//! 75 03                           jne  +3
//! 0F 05                           syscall
//! C3                              ret
//! ```
//!
//! So the displaced instructions do not have to be decoded or relocated: they
//! have to be *recognised*, and then an equivalent stub can be built from
//! scratch out of the one variable in them, the system call number. The
//! trampoline this module writes is `mov r10,rcx; mov eax,ssn; syscall; ret`,
//! which is the same call by construction rather than by copy.
//!
//! **The recognition is the safety property, and it is enforced.**
//! [`install`] refuses any target whose first sixteen bytes are not that
//! prefix, with the four system-call-number bytes wildcarded. A future Windows
//! that changes the stub shape gets a refusal and a recorded reason, not a
//! corrupted `ntdll`.
//!
//! # What it deliberately does not do
//!
//! - **It never unhooks.** A patch removed while another thread is inside the
//!   patched range is a crash with no diagnosis. The processes this runs in
//!   are short-lived test binaries, so the patch lives until the process ends.
//! - **It hooks only exported `Nt*` stubs**, never an internal `Tpp*` routine.
//!   Those are ordinary compiled functions with no fixed shape, and hooking
//!   them is exactly the case the paragraph above avoids.
//! - **It is off unless asked for twice**: the crate must carry the `trace`
//!   feature *and* the process must set `WINDOWS_THREADPOOL_TRACE_HOOKS`.
//!   Patching another module's code is not something a library may do because
//!   a consumer happened to turn tracing on.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::record;

/// The target every hook records under.
const TARGET: &str = "wfactory";

/// The fixed part of an `ntdll` syscall stub, with the four system-call-number
/// bytes at [`SSN_AT`] wildcarded.
///
/// Changing either of these is a breaking change to the recognition guard:
/// they are the whole of what makes patching safe here.
const PREFIX_HEAD: [u8; 4] = [0x4C, 0x8B, 0xD1, 0xB8];
const PREFIX_TAIL: [u8; 8] = [0xF6, 0x04, 0x25, 0x08, 0x03, 0xFE, 0x7F, 0x01];
const SSN_AT: usize = 4;
/// `jmp qword ptr [rip+0]` followed by the eight-byte destination.
const PATCH_LEN: usize = 14;

/// Why an install attempt did not happen. Recorded rather than returned,
/// because the caller is a diagnostic that must not fail a run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Refusal {
    /// `GetProcAddress` did not find the name in `ntdll`.
    NotFound = 1,
    /// The bytes at the entry point are not a syscall stub.
    NotAStub = 2,
    /// The trampoline could not be allocated.
    NoTrampoline = 3,
    /// The entry point could not be made writable.
    NotWritable = 4,
}

/// Forwarding shape for every hooked stub.
///
/// Twelve `usize` arguments, which is more than any function hooked here
/// takes (`NtCreateWorkerFactory`, the widest, takes ten). Passing more
/// arguments than the callee reads is harmless -- the extra stack slots are
/// written by this module's own frame and never read by anyone -- and it
/// removes the need for a per-function signature, which is the part of a hook
/// that is easy to get subtly wrong.
///
/// The arguments are forwarded, never interpreted, with the single exception
/// of the first: for every function here that is the worker factory handle.
type Forward = unsafe extern "system" fn(
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
) -> i32;

/// The worker factory handle, learned from the first hooked call that carries
/// one. Zero until then.
///
/// This is the reason the low-frequency hooks are worth installing even when
/// the call tracing itself is not wanted: the default process pool exposes no
/// way to reach its factory, and [`counts`] needs the handle.
static FACTORY: AtomicUsize = AtomicUsize::new(0);

/// Trampoline addresses, one per entry in the table below, in the same order.
static TRAMPOLINES: [AtomicUsize; HOOKS.len()] = [const { AtomicUsize::new(0) }; HOOKS.len()];

/// How many times each hook has been entered, counted whether or not the trace
/// is armed.
///
/// Two reasons this is not left to the trace records. It lets a capture say
/// "this hook fired ten thousand times and you are reading the last two
/// hundred" rather than leaving eviction to be inferred. And it makes the
/// facility's own guard a plain unit test: the records need
/// `WINDOWS_THREADPOOL_TRACE` set in the environment, so a test written
/// against them alone would quietly pass on every machine that did not set it,
/// which is the same thing as not having a guard.
static FIRED: [AtomicU32; HOOKS.len()] = [const { AtomicU32::new(0) }; HOOKS.len()];

/// How many times the hook with this label has been entered.
pub(crate) fn fired(label: &str) -> u32 {
    HOOKS
        .iter()
        .position(|(_, name, _)| *name == label)
        .map_or(0, |index| FIRED[index].load(Ordering::Relaxed))
}

/// Generates one hook function per stub, each recording an enter/leave pair
/// around a forward to its own trampoline.
macro_rules! hooks {
    ($($index:expr => $symbol:literal, $label:literal, $carries_handle:literal, $name:ident;)*) => {
        $(
            /// Records the call, forwards it unchanged, records the status.
            ///
            /// SAFETY: this is reached only by a jump planted over an `ntdll`
            /// syscall stub, so the caller's expectations are that stub's.
            /// Every argument is forwarded untouched to a trampoline that
            /// issues the same system call.
            #[allow(clippy::too_many_arguments)]
            unsafe extern "system" fn $name(
                a1: usize, a2: usize, a3: usize, a4: usize, a5: usize, a6: usize,
                a7: usize, a8: usize, a9: usize, a10: usize, a11: usize, a12: usize,
            ) -> i32 {
                FIRED[$index].fetch_add(1, Ordering::Relaxed);
                // Only the worker-factory calls carry a factory handle in
                // their first argument. Storing one from a stub that does not
                // -- the self-test entry takes an out-pointer there -- would
                // leave `counts` querying a garbage handle and reporting the
                // failure as if it were the factory's answer.
                if $carries_handle && a1 != 0 {
                    FACTORY.store(a1, Ordering::Relaxed);
                }
                record(TARGET, concat!($label, "-enter"), a1 as u64, a2 as u64);
                let raw = TRAMPOLINES[$index].load(Ordering::Relaxed);
                // SAFETY: non-zero only after `install` wrote an executable
                // stub there, and never cleared.
                let status = if raw == 0 {
                    0
                } else {
                    let call: Forward = unsafe { std::mem::transmute::<usize, Forward>(raw) };
                    unsafe { call(a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12) }
                };
                record(TARGET, concat!($label, "-leave"), a1 as u64, status as u32 as u64);
                if $label == "associate" {
                    // SAFETY: the call has returned, so anything it wrote
                    // through its out-parameter is finished.
                    let flag = unsafe { already_signalled(a8) };
                    record(TARGET, "associate-already-signalled", flag, a3 as u64);
                }
                status
            }
        )*

        /// Every stub this module knows how to hook: index, exported name, the
        /// label its records carry, and the function planted over it.
        const HOOKS: &[(&str, &str, Forward)] = &[
            $(($symbol, $label, $name),)*
        ];
    };
}

hooks! {
    0 => "NtWaitForWorkViaWorkerFactory", "park", true, hook_park;
    1 => "NtReleaseWorkerFactoryWorker", "release", true, hook_release;
    2 => "NtWorkerFactoryWorkerReady", "ready", true, hook_ready;
    3 => "NtSetInformationWorkerFactory", "set-info", true, hook_set_info;
    4 => "NtShutdownWorkerFactory", "shutdown", true, hook_shutdown;
    // The facility's own positive control, and not a worker-factory call at
    // all. `NtQueryTimerResolution` was chosen because nothing else in a Rust
    // process calls it: a self-test that hooked a busy stub would flood the
    // trace buffer for every test that ran after it, and the patch is never
    // removed. It is in the table rather than beside the test so that a run on
    // an unfamiliar Windows build can verify the mechanism end to end before
    // trusting what the other five report.
    5 => "NtQueryTimerResolution", "selftest", false, hook_selftest;
    // The wait registration itself, and the one entry here whose *return
    // value* is the interesting part rather than its arguments. Its first
    // argument is a wait-completion packet, not a factory, so it must not
    // teach `counts` a handle.
    6 => "NtAssociateWaitCompletionPacket", "associate", false, hook_associate;
}

/// `NtAssociateWaitCompletionPacket`'s eighth argument is a `PBOOLEAN`
/// out-parameter, `AlreadySignaled`.
///
/// **This is why the call is hooked.** When the object being waited on is
/// already signalled at the moment the packet is associated, the kernel does
/// not queue a completion -- it reports the fact through this flag and leaves
/// the caller to act on it. That is a second, entirely different delivery path
/// through the same API, taken only on a race, and a caller that mishandles it
/// loses the callback rather than delaying it. The stall under investigation
/// has exactly that shape, so whether this flag is set in a failing run is a
/// fact worth having, and it is not observable any other way.
///
/// Recorded after the call, never interpreted here.
///
/// SAFETY: `slot` is the pointer the caller passed and the call has returned,
/// so the kernel has finished writing through it. A null pointer reads as
/// absent rather than being dereferenced.
unsafe fn already_signalled(slot: usize) -> u64 {
    if slot == 0 {
        return u64::MAX;
    }
    // SAFETY: as above -- a live `BOOLEAN` the callee has just written.
    u64::from(unsafe { std::ptr::read_volatile(slot as *const u8) })
}

/// Resolve an `ntdll` export, or `None`.
///
/// Used instead of an import-library declaration so that a name this build
/// does not find is a recorded refusal at run time rather than a link failure
/// -- these are undocumented entry points, and one of them going missing on a
/// future Windows must not stop the crate from building.
fn ntdll_proc(name: &str) -> Option<usize> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

    let mut owned = name.as_bytes().to_vec();
    owned.push(0);
    // SAFETY: `ntdll.dll` is loaded in every process; a null return is handled.
    let ntdll = unsafe { GetModuleHandleA(c"ntdll.dll".as_ptr().cast()) };
    if ntdll.is_null() {
        return None;
    }
    // SAFETY: `owned` is NUL-terminated and `ntdll` is a live module handle.
    unsafe { GetProcAddress(ntdll, owned.as_ptr()) }.map(|found| found as usize)
}

/// Whether the bytes at `entry` are an `ntdll` syscall stub.
///
/// The whole safety argument for this module is that it only ever patches
/// something it has recognised, so this is the load-bearing check rather than
/// a convenience. It is separate from [`install`] so a test can assert both of
/// its directions: that it accepts a real stub, and that it rejects something
/// that is not one. A guard tested in only one direction is the recurring
/// defect this repository's instructions name.
///
/// SAFETY: `entry` must point at sixteen readable bytes.
pub(crate) unsafe fn is_syscall_stub(entry: *const u8) -> bool {
    // SAFETY: the caller guarantees sixteen readable bytes.
    let head = unsafe { std::slice::from_raw_parts(entry, 16) };
    head[..4] == PREFIX_HEAD && head[8..16] == PREFIX_TAIL
}

/// Stop every other thread in this process, run `patch`, then start them again.
///
/// Patching live code is only safe if nothing is executing the bytes being
/// written. A fourteen-byte write is not atomic and the stubs hooked here are
/// called by pool threads at arbitrary moments, so the window is small but
/// real -- and the consequence of losing that race is a thread executing half
/// of one instruction and half of another, which is a crash with no
/// diagnosis. This is what a general-purpose hooking library does for the same
/// reason.
///
/// **Nothing is recorded while threads are suspended.** A suspended thread may
/// hold the trace buffer's lock, and taking it here would deadlock the process
/// with no thread able to release it. `patch` therefore returns its outcome
/// and the caller records it afterwards.
fn with_others_suspended<T>(patch: impl FnOnce() -> T) -> T {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
        THREAD_SUSPEND_RESUME,
    };

    /// Threads handled without a second allocation. Both vectors are reserved
    /// to this before anything is suspended, and growth past it is refused
    /// rather than allocated -- see below for why that matters.
    const ROOM: usize = 512;

    // SAFETY: neither has preconditions.
    let (me, self_thread) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };

    // Phase one: enumerate. This allocates, and does so while every thread is
    // still running.
    let mut ids: Vec<u32> = Vec::with_capacity(ROOM);
    // SAFETY: no preconditions; an invalid handle is handled.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snap != INVALID_HANDLE_VALUE {
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            // SAFETY: every remaining field is a plain integer.
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: `snap` is live and `entry.dwSize` is set.
        let mut ok = unsafe { Thread32First(snap, &mut entry) } != 0;
        while ok && ids.len() < ROOM {
            if entry.th32OwnerProcessID == me && entry.th32ThreadID != self_thread {
                ids.push(entry.th32ThreadID);
            }
            // SAFETY: as above; `entry` is still initialised.
            ok = unsafe { Thread32Next(snap, &mut entry) } != 0;
        }
        // SAFETY: the snapshot is not used again.
        unsafe { CloseHandle(snap) };
    }
    let mut held: Vec<*mut core::ffi::c_void> = Vec::with_capacity(ids.len());

    // Phase two: suspend, patch, resume. **Nothing in here may allocate.**
    // Both vectors already have their capacity, so the pushes cannot grow
    // them. This is not fastidiousness: the allocator is a process-wide lock,
    // and a thread suspended while holding it can never give it back, so an
    // allocation here would deadlock the process with no thread able to run.
    // The first version of this function pushed into an unreserved vector and
    // had exactly that bug.
    for id in ids {
        // SAFETY: no preconditions; a null return is skipped.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, id) };
        if thread.is_null() {
            continue;
        }
        // SAFETY: opened for exactly this access.
        if unsafe { SuspendThread(thread) } == u32::MAX {
            // SAFETY: nothing else refers to this handle.
            unsafe { CloseHandle(thread) };
            continue;
        }
        held.push(thread);
    }

    let outcome = patch();

    for thread in held {
        // SAFETY: suspended by this function, opened for this access.
        unsafe { ResumeThread(thread) };
        // SAFETY: nothing else refers to this handle.
        unsafe { CloseHandle(thread) };
    }
    outcome
}
/// Plant a jump over one `ntdll` stub, after checking it is one.
///
/// Returns the refusal reason when nothing was patched. Every refusal leaves
/// `ntdll` exactly as it was.
fn install(index: usize) -> Result<(), Refusal> {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_READWRITE,
        VirtualAlloc, VirtualProtect,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let (symbol, _, hook) = HOOKS[index];
    let Some(entry) = ntdll_proc(symbol) else {
        return Err(Refusal::NotFound);
    };
    let entry = entry as *mut u8;

    // The recognition guard. Anything that is not the stub shape is refused
    // before a single byte is written.
    // SAFETY: an exported entry point has at least this many readable bytes.
    if !unsafe { is_syscall_stub(entry) } {
        return Err(Refusal::NotAStub);
    }
    // SAFETY: as above.
    let head = unsafe { std::slice::from_raw_parts(entry, 16) };
    let ssn = u32::from_le_bytes([
        head[SSN_AT],
        head[SSN_AT + 1],
        head[SSN_AT + 2],
        head[SSN_AT + 3],
    ]);

    // The trampoline: the same system call, rebuilt rather than copied.
    let mut stub = [0_u8; 11];
    stub[..3].copy_from_slice(&PREFIX_HEAD[..3]);
    stub[3] = 0xB8;
    stub[4..8].copy_from_slice(&ssn.to_le_bytes());
    stub[8] = 0x0F;
    stub[9] = 0x05;
    stub[10] = 0xC3;

    // SAFETY: a fresh reservation; a null return is handled.
    let page = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            4096,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        )
    };
    if page.is_null() {
        return Err(Refusal::NoTrampoline);
    }
    // SAFETY: `page` is a private writable 4 KiB reservation.
    unsafe { std::ptr::copy_nonoverlapping(stub.as_ptr(), page.cast::<u8>(), stub.len()) };
    let mut was = 0_u32;
    // SAFETY: `page` is this module's own reservation.
    unsafe { VirtualProtect(page, 4096, PAGE_EXECUTE_READ, &mut was) };
    TRAMPOLINES[index].store(page as usize, Ordering::Release);

    // The patch: `jmp qword ptr [rip+0]`, destination inline behind it.
    let mut patch = [0_u8; PATCH_LEN];
    patch[0] = 0xFF;
    patch[1] = 0x25;
    patch[6..].copy_from_slice(&(hook as usize).to_le_bytes());

    let mut before = 0_u32;
    // Everything from here to the resume must not allocate, record, or take a
    // lock: other threads are stopped and may hold any of them.
    // No suspension here: every caller wraps this in `with_others_suspended`,
    // and nesting it would restore the per-hook snapshot this batching exists
    // to remove.
    let patched = (|| {
        // SAFETY: `entry` is a live code page in this process.
        let opened =
            unsafe { VirtualProtect(entry.cast(), PATCH_LEN, PAGE_EXECUTE_READWRITE, &mut before) };
        if opened == 0 {
            return false;
        }
        // SAFETY: the page is writable for the length being written.
        unsafe { std::ptr::copy_nonoverlapping(patch.as_ptr(), entry, PATCH_LEN) };
        let mut ignored = 0_u32;
        // SAFETY: restoring the protection just changed.
        unsafe { VirtualProtect(entry.cast(), PATCH_LEN, before, &mut ignored) };
        // SAFETY: no preconditions beyond a live process handle.
        unsafe { FlushInstructionCache(GetCurrentProcess(), entry.cast(), PATCH_LEN) };
        true
    })();
    if !patched {
        return Err(Refusal::NotWritable);
    }
    Ok(())
}

/// The worker factory handle learned so far, or zero.
///
/// Exposed for the guard on the table's `carries_handle` column: a stub whose
/// first argument is not a factory handle must not be able to set this, and
/// the failure that would cause is invisible from [`counts`] alone -- querying
/// a garbage handle and querying no handle both come back `false`.
#[cfg(test)]
pub(crate) fn factory_handle() -> usize {
    FACTORY.load(Ordering::Relaxed)
}

/// Where an `ntdll` stub in the table lives, by the label it records under.
///
/// Exposed for the recogniser's guard, which has to assert that the pattern
/// still matches a real entry point on the running build and not merely a
/// byte array the test wrote itself.
#[cfg(test)]
pub(crate) fn stub_entry(label: &str) -> Option<*const u8> {
    let (symbol, _, _) = HOOKS.iter().find(|(_, name, _)| *name == label)?;
    ntdll_proc(symbol).map(|found| found as *const u8)
}

/// Issue the self-test system call, returning its status and the three timer
/// resolutions it reports, in hundred-nanosecond units.
///
/// Here rather than in the test because the call has to go through whatever is
/// currently at the entry point -- hooked or not -- which is the property
/// under test. A test that reached for its own declaration might bind to an
/// import thunk and miss the patch entirely.
#[cfg(test)]
pub(crate) fn call_selftest() -> (i32, u32, u32, u32) {
    type QueryTimerResolution = unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) -> i32;

    let Some(raw) = stub_entry("selftest") else {
        return (i32::MIN, 0, 0, 0);
    };
    // SAFETY: resolved from `ntdll` and called with its documented shape.
    let call: QueryTimerResolution =
        unsafe { std::mem::transmute::<usize, QueryTimerResolution>(raw as usize) };
    let (mut minimum, mut maximum, mut current) = (0_u32, 0_u32, 0_u32);
    // SAFETY: three live, correctly sized out-parameters.
    let status = unsafe { call(&mut minimum, &mut maximum, &mut current) };
    (status, minimum, maximum, current)
}

/// Install one hook by the label it records under.
///
/// The name-to-index step [`install_requested`] and the tests share, so a test
/// exercises the same lookup a run does rather than a parallel one.
#[cfg(test)]
pub(crate) fn install_by_label(label: &str) -> Result<(), Refusal> {
    let Some(index) = HOOKS.iter().position(|(_, name, _)| *name == label) else {
        return Err(Refusal::NotFound);
    };
    with_others_suspended(|| install(index))
}

/// Install the hooks named by `WINDOWS_THREADPOOL_TRACE_HOOKS`, once.
///
/// The variable is a comma-separated list of the labels in the table above
/// (`park`, `release`, `ready`, `set-info`, `shutdown`), or `*` for all of
/// them. It is a separate variable from `WINDOWS_THREADPOOL_TRACE` on purpose:
/// tracing observes, this modifies another module's code, and the second must
/// never follow from the first.
///
/// **`park` is the expensive one.** It brackets every worker park and unpark,
/// which is the pool's hottest path, and a measurement that installs it must
/// report the failure rate it saw alongside a run that did not -- an
/// instrument that perturbs the schedule has already changed the answer once
/// in this investigation.
///
/// Each outcome is recorded: `installed` with the index, or `refused` with the
/// index and the [`Refusal`]. A silent no-op would be indistinguishable from a
/// hook that fired zero times.
pub(crate) fn install_requested() {
    use std::sync::atomic::AtomicBool;
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let Ok(request) = std::env::var("WINDOWS_THREADPOOL_TRACE_HOOKS") else {
        return;
    };
    let wanted: Vec<&str> = request
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect();
    if wanted.is_empty() {
        return;
    }
    let mut chosen: Vec<usize> = Vec::new();
    for (index, (_, label, _)) in HOOKS.iter().enumerate() {
        if !wanted.iter().any(|want| *want == "*" || want == label) {
            continue;
        }
        chosen.push(index);
    }
    if chosen.is_empty() {
        return;
    }
    // One suspension for the whole batch, not one per hook.
    //
    // The pass is dominated by `CreateToolhelp32Snapshot`, which snapshots
    // **every thread on the machine** and is then filtered down to this
    // process -- measured at about 0.12 s each. Paying that per hook put the
    // last of six installs 0.73 s into the process. Batching makes it one
    // payment however many hooks are asked for.
    let outcomes = with_others_suspended(|| {
        let mut done = [None; HOOKS.len()];
        for &index in &chosen {
            done[index] = Some(install(index));
        }
        done
    });
    // Recorded after the resume. Nothing may take the trace lock while another
    // thread is stopped, possibly holding it.
    for index in chosen {
        match outcomes[index] {
            Some(Ok(())) => record(TARGET, "installed", index as u64, 0),
            Some(Err(why)) => record(TARGET, "refused", index as u64, why as u64),
            None => {}
        }
    }
}

/// `WorkerFactoryBasicInformation`, the one class this module reads.
const WORKER_FACTORY_BASIC_INFORMATION: u32 = 7;

/// What the factory itself believes, at the moment of the call.
///
/// **This is the reason the hooks exist.** Every measurement so far has
/// described the factory from outside -- threads parked in a dump, callbacks
/// that did or did not run. These are its own counters, and they are what
/// separates "the packet never reached the port" from "the factory has the
/// packet, has parked workers, and does not consider them available".
///
/// Returns `false` when no handle has been learned yet, which happens when no
/// hook that carries one has fired. Recorded either way, so a capture can tell
/// "asked and could not" from "never asked".
///
/// The layout below is not published by Microsoft. It is the long-standing
/// community reconstruction, and this reads only fields ahead of the first
/// pointer-sized member, so a layout that has grown at the end is still read
/// correctly. A layout that changed in the middle would not be, which is why
/// every field is recorded rather than interpreted here.
#[repr(C)]
#[derive(Default)]
struct Basic {
    timeout: i64,
    retry_timeout: i64,
    idle_timeout: i64,
    paused: u8,
    timer_set: u8,
    queued_to_ex_worker: u8,
    may_create: u8,
    create_in_progress: u8,
    inserted_into_queue: u8,
    shutdown: u8,
    _pad: u8,
    binding_count: u32,
    thread_minimum: u32,
    thread_maximum: u32,
    pending_worker_count: u32,
    waiting_worker_count: u32,
    total_worker_count: u32,
    release_count: u32,
    infinite_wait_goal: i64,
    start_routine: usize,
    start_parameter: usize,
    process_id: usize,
    stack_reserve: usize,
    stack_commit: usize,
    last_thread_creation_status: i32,
    _tail: u32,
}

type Query = unsafe extern "system" fn(usize, u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;

/// Find this process's worker factory by asking every plausible handle whether
/// it is one.
///
/// The default process pool exposes no way to reach its factory, and the hooks
/// only learn the handle once a hooked call has carried one -- which in a
/// stalled process is exactly what has not happened. So this asks instead: for
/// each candidate handle value, `NtQueryInformationWorkerFactory` succeeds only
/// on a worker factory, and answers `STATUS_OBJECT_TYPE_MISMATCH` or
/// `STATUS_INVALID_HANDLE` on anything else. The query is read-only and the
/// wrong answers are ordinary error returns, so the scan cannot disturb what it
/// is looking for -- which matters more here than usual, because the whole
/// difficulty of this investigation has been instruments that repair the fault.
///
/// Kernel handles are multiples of four, and a test process holds few of them;
/// the ceiling below covers a thousand candidates and takes well under a
/// millisecond. Every factory found is recorded, not just the first: a process
/// with a private pool as well as the default one has two, and silently
/// reporting whichever came first would be a reading rather than a measurement.
fn scan_for_factory(query: Query) -> Vec<usize> {
    /// Highest handle value tried. Handles are allocated low and densely, so
    /// this is generous for a test process rather than a tuned figure.
    const CEILING: usize = 4096;

    let mut all: Vec<usize> = Vec::new();
    let mut probe = Basic::default();
    let mut returned = 0_u32;
    let mut candidate = 4_usize;
    while candidate <= CEILING {
        // SAFETY: `probe` is a live, correctly sized buffer. An unusable
        // candidate is reported as an error status, not undefined behaviour.
        let status = unsafe {
            query(
                candidate,
                WORKER_FACTORY_BASIC_INFORMATION,
                std::ptr::from_mut(&mut probe).cast(),
                size_of::<Basic>() as u32,
                &mut returned,
            )
        };
        if status >= 0 {
            all.push(candidate);
            record(TARGET, "factory-found", candidate as u64, all.len() as u64);
        }
        candidate += 4;
    }
    record(TARGET, "factories-seen", all.len() as u64, 0);
    all
}

/// The factory's headline counts, for the scan's guard: total workers, waiting
/// workers, pending work.
///
/// Separate from [`counts`] because a guard needs values to assert on, while a
/// capture needs records. Returning them from `counts` would tempt a caller to
/// interpret a layout this module deliberately only records.
#[cfg(test)]
pub(crate) fn probe_factory() -> Option<(u32, u32, u32)> {
    let raw = ntdll_proc("NtQueryInformationWorkerFactory")?;
    // SAFETY: the name resolved in `ntdll` and this is its documented shape.
    let query: Query = unsafe { std::mem::transmute::<usize, Query>(raw) };
    let handle = *scan_for_factory(query).first()?;
    let mut info = Basic::default();
    let mut returned = 0_u32;
    // SAFETY: `info` is a live, correctly sized buffer.
    let status = unsafe {
        query(
            handle,
            WORKER_FACTORY_BASIC_INFORMATION,
            std::ptr::from_mut(&mut info).cast(),
            size_of::<Basic>() as u32,
            &mut returned,
        )
    };
    if status < 0 {
        return None;
    }
    Some((
        info.total_worker_count,
        info.waiting_worker_count,
        info.pending_worker_count,
    ))
}

/// What the factory itself believes, at the moment of the call.
///
/// **This is what every outside measurement leaves open.** Threads parked in a
/// dump and callbacks that did or did not run describe the factory from
/// outside; these are its own counters, and they separate "the packet never
/// reached the port" from "the factory has the packet, has parked workers, and
/// does not consider them available".
///
/// Returns whether anything was read. Recorded either way, so a capture can
/// tell "asked and could not" from "never asked".
///
/// The layout above is not published by Microsoft; it is the long-standing
/// community reconstruction. **It has to be exactly right, including its
/// total size**, because the query rejects a buffer whose length does not
/// match the class -- an earlier draft of this module stopped the struct after
/// the counts it wanted, and every call answered
/// `STATUS_INFO_LENGTH_MISMATCH`, which looked exactly like a process with no
/// worker factory in it. That is why the scan's guard asserts on values rather
/// than only on the call succeeding, and why the fields are recorded rather
/// than interpreted here.
pub(crate) fn counts() -> bool {
    let Some(raw) = ntdll_proc("NtQueryInformationWorkerFactory") else {
        record(TARGET, "counts-unavailable", 0, 0);
        return false;
    };
    // SAFETY: the name resolved in `ntdll` and this is its documented shape.
    let query: Query = unsafe { std::mem::transmute::<usize, Query>(raw) };
    let handle = FACTORY.load(Ordering::Relaxed);
    for (index, (_, label, _)) in HOOKS.iter().enumerate() {
        let count = fired(label);
        if count != 0 {
            // Deliberately a count and not just the records: the buffer evicts,
            // so a reader who sees two hundred `park-enter` lines needs to know
            // whether that was all of them. `fired` is the only thing in this
            // module that survives eviction.
            record(TARGET, "fired", index as u64, count as u64);
        }
    }
    // Every factory in the process, not just one. A process can hold more than
    // the default pool's -- a private pool is a second factory, and this
    // investigation's own control arms create one -- so reporting whichever
    // was found first would be picking an answer rather than measuring it.
    let mut handles = scan_for_factory(query);
    if handle != 0 && !handles.contains(&handle) {
        handles.push(handle);
    }
    if handles.is_empty() {
        record(TARGET, "counts-no-handle", 0, handle as u64);
        return false;
    }
    let mut read_any = false;
    for handle in handles {
        read_any |= read_one(query, handle);
    }
    read_any
}

/// Record one factory's counters. Every record carries the handle in its
/// second slot, so a capture from a process holding more than one factory can
/// be read apart rather than averaged into nonsense.
fn read_one(query: Query, handle: usize) -> bool {
    let mut info = Basic::default();
    let mut returned = 0_u32;
    // SAFETY: `info` is a live, correctly sized buffer; the call only writes
    // into it and reports how much it wrote.
    let status = unsafe {
        query(
            handle,
            WORKER_FACTORY_BASIC_INFORMATION,
            std::ptr::from_mut(&mut info).cast(),
            size_of::<Basic>() as u32,
            &mut returned,
        )
    };
    record(
        TARGET,
        "counts-status",
        status as u32 as u64,
        returned as u64,
    );
    if status < 0 {
        return false;
    }
    record(
        TARGET,
        "counts-total",
        info.total_worker_count as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-waiting",
        info.waiting_worker_count as u64,
        0,
    );
    record(
        TARGET,
        "counts-pending",
        info.pending_worker_count as u64,
        0,
    );
    record(
        TARGET,
        "counts-release",
        info.release_count as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-binding",
        info.binding_count as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-min-max",
        info.thread_minimum as u64,
        info.thread_maximum as u64,
    );
    record(TARGET, "counts-paused", info.paused as u64, handle as u64);
    record(
        TARGET,
        "counts-may-create",
        info.may_create as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-creating",
        info.create_in_progress as u64,
        info.inserted_into_queue as u64,
    );
    record(
        TARGET,
        "counts-shutdown",
        info.shutdown as u64,
        handle as u64,
    );
    // The field that would explain a factory refusing to make a worker, which
    // is one of the two readings this whole facility exists to separate.
    record(
        TARGET,
        "counts-last-create-status",
        info.last_thread_creation_status as u32 as u64,
        0,
    );
    record(
        TARGET,
        "counts-idle-timeout-ms",
        (info.idle_timeout / -10_000) as u64,
        (info.timeout / -10_000) as u64,
    );
    true
}
