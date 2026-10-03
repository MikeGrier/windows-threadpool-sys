// Copyright (c) Mike Grier.

//! Inline hooks on `ntdll`'s worker-factory syscall stubs.
//!
//! This exists for one investigation: a default-pool stall in which the pool
//! holds parked workers and a queued completion packet and does not put the
//! two together (see
//! [STALL-TIMELINE.md](../../STALL-TIMELINE.md)).
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
//! **With one condition, which is checked rather than assumed.** The stub
//! above is not straight-line: it tests a byte in shared data and branches
//! past `syscall` when that bit is set. The trampoline rebuilds the
//! fall-through only, so "the same call by construction" holds while the bit
//! is clear and not otherwise. It is clear on ordinary x64, which is why this
//! went unstated for several revisions -- an observation about the machine in
//! hand rather than a property of the technique. `syscall_path_is_direct`
//! reads it, and an install refuses rather than silently changing which path
//! the process takes into the kernel.
//!
//! **The recognition is the safety property, and it is enforced.**
//! [`install_batch`] refuses any target whose first sixteen bytes are not that
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
//!
//! # What enabling this costs the process, permanently
//!
//! **There is one hot-patch slot per stub per process, and this takes it.**
//! Inline hooking works by rewriting the first bytes of an entry point and
//! keeping the displaced original somewhere only the patcher knows about. The
//! technique has no way to share: a second patcher that arrives later writes
//! its own jump over the first one's, and the trampoline it builds captures
//! *this module's* jump instead of the stub Windows shipped. Nothing in the
//! mechanism detects that, because there is nothing to detect it with -- the
//! bytes at an entry point carry no record of who wrote them.
//!
//! So a process that sets `WINDOWS_THREADPOOL_TRACE_HOOKS` has given up
//! hot-patching those stubs for the rest of its life, and it cannot take that
//! back: this module never unhooks, for the reason given above. Anything else
//! that would patch the same `ntdll` entry points -- an APM or profiling
//! agent, an endpoint-security product, a Detours-style interposer, another
//! copy of this facility in a different dependency -- is in conflict with it,
//! and whichever patched second decides what the program does.
//!
//! The recogniser makes this module a well-behaved *second* patcher and does
//! nothing for the first case. [`install_batch`] refuses a target that is not an
//! unmodified stub, so arriving after somebody else yields a recorded refusal
//! rather than a corrupted chain. Arriving *before* them is the direction with
//! no defence, and it is the direction a pre-`main` installer always takes.
//!
//! **This is accepted rather than engineered around.** See [the decision in
//! DESIGN-NOTES.md](../../../../DESIGN-NOTES.md#hot-patching-is-exclusive) for why,
//! and for what a consumer who needs a different patcher should do instead.
//!
//! # The patch window, and why the supported path does not have one
//!
//! Writing fourteen bytes over code that another thread may be executing is
//! the standing hazard of this technique, and suspending the other threads
//! does not remove it: a suspended thread's instruction pointer can be
//! *inside* the range about to be overwritten, and it resumes into what is now
//! jump-displacement data.
//!
//! **That precondition is checked, on every path.** After the suspension, each
//! stopped thread's instruction pointer is read and compared against the byte
//! ranges about to be written; a thread parked inside one makes the install
//! refuse. So the hazard above is answered by an observation about this process
//! at this instant, which is the only thing that can answer it.
//!
//! It is written that way because the previous answer was prose. The supported
//! path was said not to reach the hazard "because it patches when the process
//! has no other threads", justified by installation happening from the
//! `.CRT$XCU` initialiser in [`super`] -- before `main`, and so before the test
//! harness or the pool has created a thread. Placement does not establish that
//! claim: an initialiser that runs earlier may have started threads, and in a
//! DLL the same initialiser runs at attach, inside a process that is already
//! running and may have many. The timing is still worth having, because a
//! process with one thread passes the check trivially; it is no longer what the
//! argument rests on.
//!
//! Installation is additionally confined to that initialiser rather than merely
//! arranged there, which it was not until a review found otherwise: it used to
//! sit at the end of `observe_exceptions`, a public function that `enabled`
//! calls only when the trace is armed, so a process that asked for hooks
//! without asking for the trace could reach the installer later. The initialiser
//! now installs directly and unconditionally and then seals the window;
//! [`install_requested`] refuses afterwards, and a test asserts the seal.
//!
//! `install_by_label` is `#[cfg(test)]` and does patch a live process. It takes
//! the same check, so what used to be a mitigation there is now the same proof
//! the supported path gets.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use super::record;

/// The target every hook records under.
const TARGET: &str = "wfactory";

/// Whether the one-thread window for installing hooks has closed.
///
/// The safety argument for patching `ntdll` without relocating instruction
/// pointers is that nothing else is running when the bytes are written. That
/// holds only before `main`, and only the pre-`main` initialiser knows when
/// that moment has passed -- so it says so here, and [`install_requested`]
/// refuses afterwards.
///
/// This is a flag rather than a comment because the guarantee was previously
/// carried by *which function happened to call the installer*, and that is
/// exactly the shape of rule this repository keeps getting wrong: correct
/// while nobody moves the call, silent when somebody does. `install_by_label`
/// is deliberately not subject to it -- it is `#[cfg(test)]`, it patches a
/// live process knowingly, and its hazards are documented at its definition.
static SEALED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Close the window in which hooks may be installed.
///
/// Called once, by the pre-`main` initialiser, immediately after it has given
/// [`install_requested`] its only chance to run.
pub(crate) fn seal_installation_window() {
    SEALED.store(true, Ordering::SeqCst);
}

/// Whether the installation window has closed.
///
/// Exists so a test can assert that the pre-`main` initialiser actually sealed
/// it. Without that assertion the seal is a line of code nothing exercises,
/// and the guarantee would be back to resting on a call nobody checks.
#[cfg(test)]
pub(crate) fn installation_window_sealed() -> bool {
    SEALED.load(Ordering::SeqCst)
}

/// The fixed part of an `ntdll` syscall stub, with the four system-call-number
/// bytes at [`SSN_AT`] wildcarded.
///
/// Changing either of these is a breaking change to the recognition guard:
/// they are the whole of what makes patching safe here.
const PREFIX_HEAD: [u8; 4] = [0x4C, 0x8B, 0xD1, 0xB8];
const PREFIX_TAIL: [u8; 8] = [0xF6, 0x04, 0x25, 0x08, 0x03, 0xFE, 0x7F, 0x01];
const SSN_AT: usize = 4;

/// The shared-data byte the recognised stub tests, derived from the pattern
/// that recognises it rather than written out a second time.
///
/// `PREFIX_TAIL` *is* `test byte ptr [7FFE0308h], 1`: bytes 3..7 are the
/// little-endian address and byte 7 is the mask. Taking both from there means
/// the check below cannot drift from the shape it is checking.
const SHARED_FLAG_ADDRESS: usize = u32::from_le_bytes([
    PREFIX_TAIL[3],
    PREFIX_TAIL[4],
    PREFIX_TAIL[5],
    PREFIX_TAIL[6],
]) as usize;
const SHARED_FLAG_MASK: u8 = PREFIX_TAIL[7];

/// Whether the recognised stub's branch falls through to `syscall`.
///
/// **This is the trampoline's unstated premise, made a checked one.** The stub
/// does not go straight to `syscall`: it tests a byte in shared data and takes
/// an alternate path when that bit is set. The trampoline this module builds
/// is `mov r10,rcx; mov eax,ssn; syscall; ret`, which reproduces only the
/// fall-through -- so it is the same call by construction *when the bit is
/// clear*, and a different one when it is not. The recogniser matches those
/// test bytes and then ignores what they select.
///
/// The bit is clear on ordinary x64, which is why nothing has noticed. That is
/// an observation about this machine, not a property of the technique, and
/// binding to it silently is what the house rule against depending on
/// incidental behaviour forbids. So it is read, and an install refuses when it
/// is set rather than quietly rewriting which path the process takes into the
/// kernel.
fn syscall_path_is_direct() -> bool {
    // SAFETY: `KUSER_SHARED_DATA` is mapped read-only at a fixed address in
    // every user-mode process on Windows, and this reads one byte inside it.
    // Volatile because the kernel owns the page and may change it.
    let flag = unsafe { std::ptr::read_volatile(SHARED_FLAG_ADDRESS as *const u8) };
    flag & SHARED_FLAG_MASK == 0
}
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
    /// The trampoline page could not be made executable.
    ///
    /// Separate from [`NoTrampoline`](Self::NoTrampoline), which is a failed
    /// allocation: here the page exists and holds the rebuilt stub, but the
    /// call that would let it be *executed* did not succeed. Installing anyway
    /// would plant a jump into memory the processor refuses to run.
    TrampolineNotExecutable = 5,
    /// The stub's branch does not fall through to `syscall` on this system.
    ///
    /// The trampoline reproduces the fall-through only, so it would be a
    /// different call from the one it replaced. See
    /// [`syscall_path_is_direct`].
    IndirectSyscallPath = 7,
    /// Some thread could not be stopped, so the process was not quiesced.
    ///
    /// The entire safety argument for writing over live code is that nothing
    /// is executing the bytes being written. A thread this module failed to
    /// open or suspend is still running, so that argument does not hold and
    /// nothing is patched.
    NotQuiesced = 6,
}

/// Drop **this** thread from the enumeration, to stand in for one created after
/// it. Zero omits nothing.
///
/// The sweep in [`quiesce_once`] refuses when it meets a thread the snapshot did
/// not list. The real way to produce one is to have a listed thread spawn
/// another before it is itself suspended, which is a race no test can land on
/// demand -- so the *effect* is produced instead, by removing an id the snapshot
/// did list. From the sweep's side the two are identical: a live thread it did
/// not suspend.
///
/// **Keyed to one id rather than "drop the last one", and both halves of that
/// matter.** An unkeyed flag perturbs whichever install happens to be running,
/// and the registry of test threads is process-wide. Worse, it is not even
/// faithful: an arbitrary dropped id may belong to a thread that has since
/// exited, and `install_batch` retries sixteen times -- so the stranger stops
/// existing and the install succeeds. Measured at 32 test threads: 4 failures in
/// 20 runs, every one of them `Ok(())` where a refusal was required. The caller
/// names a thread it is itself keeping alive, so the stranger is there for every
/// attempt.
///
/// Without this the guard has no test at all. Measured, before it existed:
/// making the sweep blind (`known = true`) left all 292 lib tests passing, so a
/// sabotage of it would have been scored SURVIVED and the manifest entry
/// claiming otherwise would have been a false claim.
#[cfg(test)]
pub(super) static OMIT_THREAD_FROM_SNAPSHOT: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// `ntdll!NtGetNextThread`, the allocation-free way to walk this process's
/// threads.
///
/// `(ProcessHandle, ThreadHandle, DesiredAccess, HandleAttributes, Flags,
/// NewThreadHandle)`. Used only by [`quiesce_once`], which needs an enumeration
/// it can run with other threads suspended -- see the sweep there for why a
/// second toolhelp snapshot would not do.
type NtGetNextThread = unsafe extern "system" fn(
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    u32,
    u32,
    u32,
    *mut *mut core::ffi::c_void,
) -> i32;

/// Compare two labels in a `const`, which `==` on `&str` cannot do here.
///
/// Exists only for the arity assertions generated beside the hook table.
const fn label_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// How many arguments the forwarding shape carries.
///
/// Twelve, which is more than any function hooked here takes
/// (`NtCreateWorkerFactory`, the widest, takes ten).
const FORWARD_ARITY: usize = 12;

/// Forwarding shape for every hooked stub.
///
/// Passing more arguments than the callee reads is harmless -- the extra stack
/// slots are written by this module's own frame and never read by anyone -- so
/// one shape serves every trampoline.
///
/// **This reasoning is about the outgoing call only, and an earlier revision of
/// this comment used it to justify the incoming side too.** It does not carry:
/// a hook *declared* with twelve arguments and planted over a three-argument
/// stub reads incoming stack slots the caller never had to supply. Each hook is
/// now declared with its own stub's arity where this crate can establish it;
/// see the note under the table.
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
        .position(|(_, name)| *name == label)
        .map_or(0, |index| FIRED[index].load(Ordering::Relaxed))
}

/// Maps one argument name to its type, so a parameter *list* can become a type
/// list inside the `hooks!` table below. Every hooked argument is a `usize`.
macro_rules! arg_ty {
    ($ignored:ident) => {
        usize
    };
}

/// Generates one hook function per stub, each recording an enter/leave pair
/// around a forward to its own trampoline.
macro_rules! hooks {
    ($($index:expr => $symbol:literal, $label:literal, $carries_handle:literal, $name:ident,
       ($($arg:ident),*);)*) => {
        $(
            /// Records the call, forwards it unchanged, records the status.
            ///
            /// **Declared with the arity of the stub it is planted over**, which
            /// the table supplies. Forwarding more arguments than a callee reads
            /// is harmless and this module relies on it; *receiving* more than
            /// the caller passed is a different thing and is not harmless. A
            /// twelve-argument hook planted over a three-argument stub reads
            /// incoming stack slots the caller never had to supply: on x64 those
            /// land inside the caller's own frame, so it reads stale bytes
            /// rather than faulting, which is why it was never seen to
            /// misbehave and why it was still wrong.
            ///
            /// SAFETY: this is reached only by a jump planted over an `ntdll`
            /// syscall stub, so the caller's expectations are that stub's.
            /// Every argument is forwarded untouched to a trampoline that
            /// issues the same system call.
            #[allow(clippy::too_many_arguments)]
            unsafe extern "system" fn $name($($arg: usize),*) -> i32 {
                FIRED[$index].fetch_add(1, Ordering::Relaxed);
                // Collected so the body below can be written once for every
                // arity. Indexing past the end is impossible: the reads are
                // `get`, and the pad is what the fixed-width forward needs.
                let args = [$($arg),*];
                let a1 = args[0];
                let a2 = args.get(1).copied().unwrap_or(0);
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
                // stub there. It is cleared only by `discard_prepared`, and
                // only on the path where the slot was empty beforehand -- which
                // means no stub was ever patched to reach this body, so a clear
                // cannot race a call that is already here.
                let status = if raw == 0 {
                    0
                } else {
                    // Padded out to the fixed forwarding width. This is the
                    // direction that *is* harmless: the trampoline reads only
                    // what its system call takes, and the slots beyond that are
                    // written by this frame and read by nobody.
                    let mut full = [0usize; FORWARD_ARITY];
                    full[..args.len()].copy_from_slice(&args);
                    let call: Forward = unsafe { std::mem::transmute::<usize, Forward>(raw) };
                    unsafe {
                        call(
                            full[0], full[1], full[2], full[3], full[4], full[5], full[6],
                            full[7], full[8], full[9], full[10], full[11],
                        )
                    }
                };
                record(TARGET, concat!($label, "-leave"), a1 as u64, status as u32 as u64);
                if $label == "associate" && status >= 0 {
                    // Only on success, and that condition is the whole of the
                    // safety argument. `AlreadySignaled` is an out-parameter
                    // the kernel writes through; a call that failed need not
                    // have written it and need not have validated the pointer
                    // at all. Reading regardless turns an ordinary error
                    // return -- a bad handle, say -- into an access violation
                    // raised by the *instrument*, which is the one thing a
                    // diagnostic must never do to the program it observes.
                    //
                    // A failed association has nothing to report here anyway:
                    // no packet was associated, so there was no object whose
                    // signalled state could have been observed. The `-leave`
                    // record above already carries the status.
                    //
                    // SAFETY: the call returned success, so the kernel wrote
                    // through the out-parameter and has finished with it.
                    // `get` rather than indexing, because this body is compiled
                    // for every arity in the table and a three-argument hook has
                    // no eighth slot. That it is never `None` for the hook that
                    // actually takes this branch is asserted below, at compile
                    // time, so the fallback cannot quietly swallow a mistake.
                    if let (Some(&signalled), Some(&packet)) = (args.get(7), args.get(2)) {
                        // SAFETY: the call returned success, so the kernel has
                        // written through the out-parameter and finished with
                        // it; the reader refuses anything that cannot be one.
                        match unsafe { already_signalled(signalled) } {
                            Signalled::Flag(set) => record(
                                TARGET,
                                "associate-already-signalled",
                                u64::from(set),
                                packet as u64,
                            ),
                            Signalled::Absent => record(
                                TARGET,
                                "associate-already-signalled",
                                NO_FLAG,
                                packet as u64,
                            ),
                            // The arity-drift signal. Carries the raw argument
                            // rather than a flag, because the finding is what
                            // the eighth slot held instead -- a small integer
                            // says it is now some other parameter.
                            Signalled::Implausible => record(
                                TARGET,
                                "associate-flag-implausible",
                                signalled as u64,
                                packet as u64,
                            ),
                        }
                    }
                }
                status
            }
        )*

        /// No stub may declare more arguments than the forward can carry.
        ///
        /// The pad above would panic at the `copy_from_slice` otherwise, inside
        /// a hook planted over a live system call -- which is the worst place in
        /// this crate for a runtime failure. A table entry that outgrows the
        /// forwarding width fails the build instead.
        $(
            const _: () = assert!(
                [$(stringify!($arg)),*].len() <= FORWARD_ARITY,
                "a hook declares more arguments than the forwarding shape carries"
            );
        )*

        /// The `associate` hook reads its eighth argument, so it must have one.
        ///
        /// Without this, narrowing that entry's arity below eight would compile:
        /// the read is a `get` whose `None` arm does nothing, so the record
        /// would simply stop being emitted and every test of it would still
        /// pass. The check belongs at the arity, which is the thing that would
        /// change.
        $(
            const _: () = assert!(
                !label_eq($label, "associate") || [$(stringify!($arg)),*].len() >= 8,
                "the associate hook reads AlreadySignaled, its eighth argument"
            );
        )*

        /// Every stub this module knows how to hook: exported name and the
        /// label its records carry, in index order.
        ///
        /// The replacement function is **not** stored here. Each one now has its
        /// own stub's signature, so there is no single function type the column
        /// could have; its address is reached through [`hook_address`] instead.
        const HOOKS: &[(&str, &str)] = &[
            $(($symbol, $label),)*
        ];

        /// The address of the function planted over the stub at `index`.
        ///
        /// A function rather than a table column because casting a function to
        /// an integer is not something a `const` can do; at run time it is
        /// ordinary. Returns zero for an index outside the table, which cannot
        /// happen -- every caller derives its index from `HOOKS` -- and is a
        /// refusal rather than a panic if it ever did, because this runs beside
        /// code that patches live system calls.
        fn hook_address(index: usize) -> usize {
            match index {
                $($index => {
                    // Bound to the hook's own pointer type before the integer
                    // cast: a function *item* cannot be cast to an integer
                    // directly, and there is no longer one shared signature to
                    // route it through.
                    let planted: unsafe extern "system" fn($(arg_ty!($arg)),*) -> i32 = $name;
                    planted as usize
                })*
                _ => 0,
            }
        }
    };
}

// The final column is the stub's argument list, which becomes the hook's own
// signature. Where a stub's arity is not something this crate can establish, it
// is left at the full forwarding width -- see the note below the table.
hooks! {
    0 => "NtWaitForWorkViaWorkerFactory", "park", true, hook_park,
        (a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12);
    1 => "NtReleaseWorkerFactoryWorker", "release", true, hook_release,
        (a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12);
    2 => "NtWorkerFactoryWorkerReady", "ready", true, hook_ready,
        (a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12);
    3 => "NtSetInformationWorkerFactory", "set-info", true, hook_set_info,
        (a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12);
    4 => "NtShutdownWorkerFactory", "shutdown", true, hook_shutdown,
        (a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12);
    // The facility's own positive control, and not a worker-factory call at
    // all. `NtQueryTimerResolution` was chosen because nothing else in a Rust
    // process calls it: a self-test that hooked a busy stub would flood the
    // trace buffer for every test that ran after it, and the patch is never
    // removed. It is in the table rather than beside the test so that a run on
    // an unfamiliar Windows build can verify the mechanism end to end before
    // trusting what the other five report.
    //
    // Three arguments, and this crate can say so from its own code rather than
    // from a header it does not own: `call_selftest` invokes the stub through a
    // three-argument pointer and asserts the kernel wrote all three
    // out-parameters.
    5 => "NtQueryTimerResolution", "selftest", false, hook_selftest, (a1, a2, a3);
    // The wait registration itself, and the one entry here whose *return
    // value* is the interesting part rather than its arguments. Its first
    // argument is a wait-completion packet, not a factory, so it must not
    // teach `counts` a handle.
    //
    // Eight arguments, which this module already depends on: `AlreadySignaled`
    // is the eighth, and `already_signalled` dereferences it. A hook declaring
    // fewer could not read it at all.
    6 => "NtAssociateWaitCompletionPacket", "associate", false, hook_associate,
        (a1, a2, a3, a4, a5, a6, a7, a8);
}

/// Why five of the seven stubs above still take the full forwarding width.
///
/// Not an oversight, and not the same judgement as the two that do not. The
/// five worker-factory calls are undocumented, and at least one of them --
/// `NtWaitForWorkViaWorkerFactory` -- has been described with different
/// argument counts on different Windows versions. Declaring too *many*
/// arguments reads stale bytes from the caller's frame and forwards values the
/// kernel ignores. Declaring too *few* silently drops a real argument, and on
/// that particular stub the result would be every pool worker parking with a
/// zeroed parameter: a far worse failure, in the exact code path this facility
/// exists to observe.
///
/// So the rule is: narrow a hook to a stub's real arity only where this crate
/// can establish that arity from something it owns. `selftest` qualifies
/// because `call_selftest` calls it directly; `associate` qualifies because
/// `already_signalled` reads its eighth argument. Nothing here establishes the
/// other five, so they keep the conservative upper bound.
///
/// Narrowing them needs a source for the arity that is good on every supported
/// Windows version, which is a different kind of work from this change.
const _ARITY_RATIONALE: () = ();

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
/// absent rather than being dereferenced, and a value that cannot be a user
/// address is reported rather than dereferenced -- see [`Signalled`].
pub(super) unsafe fn already_signalled(slot: usize) -> Signalled {
    if slot == 0 {
        return Signalled::Absent;
    }
    if !(LOWEST_USER_ADDRESS..=HIGHEST_USER_ADDRESS).contains(&slot) {
        return Signalled::Implausible;
    }
    // SAFETY: as above -- a live `BOOLEAN` the callee has just written.
    match unsafe { std::ptr::read_volatile(slot as *const u8) } {
        0 => Signalled::Flag(false),
        1 => Signalled::Flag(true),
        _ => Signalled::Implausible,
    }
}

/// What this hook's eighth argument turned out to be.
///
/// **Three outcomes rather than a flag, and the third one is the point.** This
/// is the only hook that *interprets* a high-numbered argument rather than
/// forwarding it, so it is the only one whose correctness depends on the stub's
/// arity being what the table says. If `NtAssociateWaitCompletionPacket` ever
/// gains or loses a parameter, the eighth slot stops being `AlreadySignaled`
/// and becomes some other argument -- and dereferencing *that* raises an access
/// violation inside the instrument, which this module holds to be the one thing
/// a diagnostic must never do to the program it observes.
///
/// So the value is checked twice before it is believed: the slot must be a
/// possible user-mode address, and what it points at must be a `BOOLEAN`.
/// Neither check can produce a false alarm -- the kernel writes 0 or 1 through
/// a pointer it has just validated -- so an `Implausible` answer means the
/// argument is not the one this hook thinks it is.
///
/// **What this does not establish.** A plausible-looking address may still be
/// unmapped, and no cheap check run on every wait registration can prove
/// otherwise. What the range rejects is the overwhelmingly likely shape of a
/// shifted argument: a small integer -- a status, a count, a flag -- which is
/// what the other parameters of this call actually hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Signalled {
    /// The caller passed no out-parameter. Nothing was read.
    Absent,
    /// The kernel's answer: whether the object was already signalled.
    Flag(bool),
    /// Not an `AlreadySignaled` pointer. Treat the stub's arity as suspect.
    Implausible,
}

/// The lowest address a user-mode pointer can have: the first 64 KiB of the
/// address space is permanently unmapped, which is what makes a small integer
/// distinguishable from a pointer.
const LOWEST_USER_ADDRESS: usize = 0x1_0000;

/// The highest user-mode address on x64 Windows; above this is kernel space or
/// non-canonical.
const HIGHEST_USER_ADDRESS: usize = 0x7FFF_FFFF_FFFF;

/// Reported in place of the flag when the caller passed no out-parameter.
///
/// Named rather than written at the record site: it shares a field with a real
/// `0`/`1` answer, so a reader needs the two to be distinguishable.
const NO_FLAG: u64 = u64::MAX;

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
/// a convenience. It is separate from [`install_batch`] so a test can assert both of
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
///
/// Returns `None` without running `patch` when any thread could not be stopped.
/// Every thread this call did suspend is resumed either way.
/// Attempts [`install_batch`] makes before it refuses.
///
/// One attempt refuses whenever any thread cannot be stopped, and a thread that
/// is *terminating* cannot be: `SuspendThread` answers `ERROR_ACCESS_DENIED`
/// for one, on a handle that `OpenThread` had just returned for the same id.
/// That is not the `OpenThread` case handled in the suspend loop below and must
/// not be folded into it -- an id that names nothing is gone, whereas a
/// terminating thread may still be executing its exit path, which is the
/// property the refusal is about.
///
/// So the refusal stands and the attempt is repeated instead. Each one resumes
/// everything it stopped before returning, so a retry begins with nothing
/// suspended and takes the lock again, and the thread that could not be stopped
/// is given a moment to finish leaving.
///
/// Measured: running this crate's own lib tests at 32 test threads, 13 of 30
/// runs refused the self-test install, every one of them reporting
/// `SuspendThread` / `ERROR_ACCESS_DENIED` with the enumeration intact.
pub(super) const ATTEMPTS: usize = 16;

/// Time given to a terminating thread between attempts.
pub(super) const SETTLE: std::time::Duration = std::time::Duration::from_millis(1);

/// One attempt: stop every other thread, run `patch` if and only if every one
/// of them stopped, resume them all.
///
/// **The retry around this lives in [`install_batch`], not here**, and that is
/// the point rather than an accident of layout. The target pages must be made
/// writable before the suspension and restored after it, because
/// `VirtualProtect` cannot be called inside the window. A retry wrapped around
/// this function alone would therefore hold `ntdll`'s code pages
/// `PAGE_EXECUTE_READWRITE` across every attempt -- on the order of a second
/// with every other thread running, since each attempt takes a fresh thread
/// snapshot -- where one attempt holds them for microseconds. The caller opens
/// and restores per attempt so the writable window stays the size it was before
/// the retry existed.
fn quiesce_once<T>(ranges: &[(usize, usize)], patch: impl FnOnce() -> T) -> Option<T> {
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread,
        THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
    };

    /// `CONTEXT_CONTROL` for x86-64: the group holding `Rip`.
    ///
    /// Spelled out because `windows-sys` exposes the architecture's control
    /// flag under a name that varies by target, and only the instruction
    /// pointer is wanted here.
    const CONTEXT_CONTROL_AMD64: u32 = 0x0010_0001;

    /// Threads handled without a second allocation. Both vectors are reserved
    /// to this before anything is suspended, and a process with more threads
    /// than this makes the whole install refuse rather than either allocating
    /// in the suspended window or quietly suspending a prefix -- see below for
    /// why both of those matter.
    const ROOM: usize = 512;

    /// Serializes every path into this function, against itself.
    ///
    /// Two threads here at once can each enumerate the other and then suspend
    /// it. Neither is then running to resume the other, and the process stops
    /// with no thread able to release it. The window is the interval in which
    /// `SuspendThread` has returned on one thread but the target has not yet
    /// stopped executing -- the API is documented as not guaranteeing the
    /// suspension is complete when it returns.
    ///
    /// This lock is held by the suspending thread, which never suspends itself
    /// (the enumeration filters out its own id), so the holder is always
    /// running and a thread blocked here is simply waiting rather than
    /// deadlocked.
    ///
    /// It is placed here rather than at the callers because that is what makes
    /// the rule hold for all of them: `install_requested` had an `AtomicBool`
    /// that stops it running *twice*, which is idempotence and not mutual
    /// exclusion, and `install_by_label` had nothing. Serializing the thing
    /// that suspends is one site; guarding each entry point is a rule to
    /// remember every time one is added.
    ///
    /// Poison is recovered rather than propagated: a panic inside a previous
    /// `patch` says nothing about whether it is safe to suspend threads now,
    /// and refusing every later install because of it would disable the
    /// facility for the rest of the process.
    static PATCHING: Mutex<()> = Mutex::new(());
    let _serialized = PATCHING.lock().unwrap_or_else(|poison| poison.into_inner());

    // SAFETY: neither has preconditions.
    let (me, self_thread) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };

    // Phase one: enumerate. This allocates, and does so while every thread is
    // still running.
    //
    // **The enumeration fails closed**, and did not until a review pointed at
    // it. An earlier revision refused when a thread could not be opened or
    // suspended, and left every way of failing to *find* the threads reporting
    // success: a snapshot that could not be taken, an enumeration that could
    // not be started, and a thread count past `ROOM` all left `ids` empty or
    // short, which then read as "every thread is suspended" and patched
    // fourteen bytes of live code with nothing stopped at all. That is a
    // strictly worse version of the case that had been fixed, one block above
    // it.
    let mut ids: Vec<u32> = Vec::with_capacity(ROOM);
    let mut enumerated = false;
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
        // A process always has at least this thread, so a first call that
        // reports nothing is a failed enumeration rather than an empty one.
        let started = ok;
        let mut overflowed = false;
        while ok {
            if entry.th32OwnerProcessID == me && entry.th32ThreadID != self_thread {
                if ids.len() == ROOM {
                    // Refuse rather than truncate. Growing the vector here
                    // would allocate, and a short list is the dangerous answer
                    // dressed as a complete one.
                    overflowed = true;
                    break;
                }
                ids.push(entry.th32ThreadID);
            }
            // SAFETY: as above; `entry` is still initialised.
            ok = unsafe { Thread32Next(snap, &mut entry) } != 0;
        }
        // SAFETY: the snapshot is not used again.
        unsafe { CloseHandle(snap) };
        enumerated = started && !overflowed;
    }
    // Stands in for a thread created between the snapshot and the suspension.
    // Applied after `enumerated` is decided, so this weakens what gets suspended
    // without pretending the enumeration itself failed. `retain` reorders
    // nothing and allocates nothing.
    #[cfg(test)]
    {
        let omitted = OMIT_THREAD_FROM_SNAPSHOT.load(Ordering::SeqCst);
        if omitted != 0 {
            ids.retain(|&id| id != omitted);
        }
    }

    // Resolved **now**, while allocation is still safe, because the sweep that
    // uses it runs with threads stopped. `ntdll_proc` allocates a NUL-terminated
    // copy of the name and calls `GetProcAddress`, which takes the loader lock;
    // either one taken inside the suspended window can deadlock against a thread
    // suspended while holding it, which is the hazard this whole phase is
    // arranged around.
    let next_thread: Option<NtGetNextThread> = ntdll_proc("NtGetNextThread").map(|address| {
        // SAFETY: the export exists in every supported `ntdll` and its
        // signature is the one declared on the type alias.
        unsafe { std::mem::transmute::<usize, NtGetNextThread>(address) }
    });

    let mut held: Vec<*mut core::ffi::c_void> = Vec::with_capacity(ids.len());

    // Phase two: suspend, patch, resume. **Nothing in here may allocate.**
    // Both vectors already have their capacity, so the pushes cannot grow
    // them. This is not fastidiousness: the allocator is a process-wide lock,
    // and a thread suspended while holding it can never give it back, so an
    // allocation here would deadlock the process with no thread able to run.
    // The first version of this function pushed into an unreserved vector and
    // had exactly that bug.
    // Iterated by reference, not consumed: `for id in ids` would drop the
    // vector at the end of this loop, which is a `free` -- and so an allocator
    // lock -- taken with every other thread already stopped. It is dropped at
    // the end of the function instead, once they are running again.
    // A thread this loop cannot stop is a thread that is still running, and
    // "nothing is executing the bytes being written" is the whole of why this
    // is allowed to write over live code. Both failures used to `continue`,
    // which discarded the precondition rather than the thread and patched
    // anyway.
    //
    // **One of the two failures is benign and has to be told apart**, or this
    // refuses almost every live install: the snapshot is taken while the
    // process runs, so a thread listed in it can exit before `OpenThread`
    // reaches it, and a pool that is starting and finishing workers does that
    // routinely. `ERROR_INVALID_PARAMETER` is how `OpenThread` reports a
    // thread id that no longer names anything -- and a thread that does not
    // exist is not executing the bytes about to be written, which is exactly
    // the property being established. Any other failure is a thread that is
    // there and could not be opened, and that refuses.
    //
    // Measured: refusing both made `install_by_label` fail in an ordinary test
    // process, which is what sent this back for the distinction.
    let mut quiesced = enumerated;
    // Nothing is suspended at all when the enumeration failed: stopping every
    // thread and then declining to patch would be a perturbation bought for
    // nothing, in the window this function exists to keep empty.
    for &id in if enumerated { &ids[..] } else { &[][..] } {
        // SAFETY: no preconditions; a null return is handled.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, 0, id) };
        if thread.is_null() {
            // SAFETY: no preconditions; reports the call immediately above.
            if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
                continue;
            }
            quiesced = false;
            break;
        }
        // SAFETY: opened for exactly this access.
        if unsafe { SuspendThread(thread) } == u32::MAX {
            // SAFETY: nothing else refers to this handle.
            unsafe { CloseHandle(thread) };
            quiesced = false;
            break;
        }
        held.push(thread);
    }

    // Every thread the snapshot listed is stopped. **That is not the same as
    // every thread being stopped**, and the difference is the one this block
    // closes. The snapshot is taken while the process runs, and the suspend loop
    // above runs with its targets still running, so a thread listed there can
    // create another before it is itself suspended. The newcomer is in neither
    // the snapshot nor `held`: it is running, and the instruction-pointer check
    // below never sees it, because that walks only threads that were suspended.
    //
    // So the thread set is swept again here, now that nothing we know of is
    // running, and any stranger refuses the attempt. `install_batch` retries, by
    // which time the newcomer is in the fresh snapshot and gets suspended like
    // any other.
    //
    // **`NtGetNextThread` rather than a second toolhelp snapshot**, and that is
    // forced rather than chosen. Re-enumerating with
    // `CreateToolhelp32Snapshot` allocates, and an allocation here deadlocks the
    // process against a thread suspended while holding the allocator lock --
    // the hazard this phase is built to avoid, which an earlier version of this
    // function already hit once. The `Nt` walk takes a handle and a thread id
    // and touches no user-mode heap or lock.
    //
    // Fails closed when the export cannot be resolved, matching the rest of this
    // module: an unverifiable precondition is not a satisfied one.
    if quiesced {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetThreadId};
        /// `THREAD_QUERY_LIMITED_INFORMATION`, all `GetThreadId` needs.
        const QUERY_LIMITED: u32 = 0x0800;

        match next_thread {
            None => quiesced = false,
            Some(next) => {
                // SAFETY: a pseudo-handle with no lifetime and no preconditions.
                let process = unsafe { GetCurrentProcess() };
                let mut cursor: *mut core::ffi::c_void = std::ptr::null_mut();
                loop {
                    let mut found: *mut core::ffi::c_void = std::ptr::null_mut();
                    // SAFETY: `process` is this process, `cursor` is null on the
                    // first call and a handle this loop owns afterwards, and
                    // `found` is a live local the call writes through.
                    let status = unsafe { next(process, cursor, QUERY_LIMITED, 0, 0, &mut found) };
                    if !cursor.is_null() {
                        // SAFETY: owned by this loop and not used again; the
                        // walk took its own reference for the next step.
                        unsafe { CloseHandle(cursor) };
                    }
                    if status < 0 || found.is_null() {
                        // A non-success status ends the walk. Only
                        // STATUS_NO_MORE_ENTRIES is expected, and distinguishing
                        // it would not change what happens: the sweep has seen
                        // every thread it is going to see.
                        break;
                    }
                    // SAFETY: `found` is a live thread handle opened for query.
                    let id = unsafe { GetThreadId(found) };
                    let known = id == self_thread || ids.contains(&id);
                    if !known {
                        quiesced = false;
                        // SAFETY: owned here and not used again.
                        unsafe { CloseHandle(found) };
                        break;
                    }
                    cursor = found;
                }
                if !cursor.is_null() {
                    // SAFETY: the last handle the walk produced, owned here.
                    unsafe { CloseHandle(cursor) };
                }
            }
        }
    }

    // Every other thread is stopped. That is still not enough to write over
    // live code: a thread can be suspended with its instruction pointer
    // *inside* the bytes about to be replaced, and it would resume into what is
    // by then jump-displacement data.
    //
    // This used to be answered by asserting that the supported path installs
    // from a `.CRT$XCU` initialiser, "where this process still has exactly one
    // thread". Placement does not establish that: an earlier initialiser may
    // have started threads, and in a DLL the same initialiser runs at attach,
    // inside a process that is already running. The claim was prose, and prose
    // is not a rung -- so the precondition is checked here instead, and the
    // install refuses when it does not hold.
    if quiesced && !ranges.is_empty() {
        for &thread in &held {
            // Aligned here rather than trusted to the binding. `GetThreadContext`
            // documents a 16-byte alignment requirement for this structure on
            // this architecture, and `align_of::<CONTEXT>()` from `windows-sys`
            // measures **8**. Passing the under-aligned buffer happened to work
            // when this was written, which is the kind of incidental behaviour
            // that is not a contract; the wrapper asks for what the API asks
            // for.
            #[repr(C, align(16))]
            struct Aligned(CONTEXT);

            // Zeroed is a valid starting state once `ContextFlags` says which
            // groups to fill.
            let mut context = Aligned(unsafe { std::mem::zeroed() });
            context.0.ContextFlags = CONTEXT_CONTROL_AMD64;
            // SAFETY: the thread is suspended and was opened for this access;
            // `context` is a live, correctly aligned buffer.
            if unsafe { GetThreadContext(thread, &mut context.0) } == 0 {
                // A thread whose instruction pointer cannot be read is a thread
                // this cannot clear, and an unreadable answer is not a safe one.
                quiesced = false;
                break;
            }
            let rip = context.0.Rip as usize;
            if ranges.iter().any(|&(start, end)| rip >= start && rip < end) {
                quiesced = false;
                break;
            }
        }
    }

    let outcome = if quiesced { Some(patch()) } else { None };

    for &thread in &held {
        // SAFETY: suspended by this function, opened for this access.
        unsafe { ResumeThread(thread) };
        // SAFETY: nothing else refers to this handle.
        unsafe { CloseHandle(thread) };
    }
    // Both vectors are dropped here, after the resume, for the reason given at
    // the suspend loop above.
    drop(held);
    drop(ids);
    outcome
}
/// Everything one install needs, resolved and allocated before any thread is
/// suspended.
///
/// This type exists because of what must *not* happen inside the suspended
/// window. See [`install_batch`].
struct Prepared {
    /// The stub being patched.
    entry: *mut u8,
    /// The fourteen bytes to store over it.
    patch: [u8; PATCH_LEN],
    /// Which hook this is, so a discarded preparation can find its slot.
    index: usize,
    /// The trampoline page this preparation allocated and published.
    trampoline: *mut core::ffi::c_void,
    /// What `TRAMPOLINES[index]` held before this preparation published over
    /// it. Zero means no install had ever succeeded for this hook, which is
    /// what makes the page safe to release if the batch is refused -- see
    /// [`discard_prepared`].
    previous: usize,
}

/// Resolve, recognise, and make ready to patch -- all of it outside any
/// suspension.
///
/// Returns the refusal reason when nothing can be patched. Every refusal leaves
/// `ntdll` exactly as it was.
fn prepare(index: usize) -> Result<Prepared, Refusal> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc,
        VirtualFree, VirtualProtect,
    };

    let (symbol, _) = HOOKS[index];
    let hook = hook_address(index);
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
    // Recognising the shape is not enough: the shape contains a branch, and
    // the trampoline rebuilds only one side of it.
    if !syscall_path_is_direct() {
        return Err(Refusal::IndirectSyscallPath);
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
    let executable = unsafe { VirtualProtect(page, 4096, PAGE_EXECUTE_READ, &mut was) };
    if executable == 0 {
        // Checked, where it used to be discarded. The trampoline pointer was
        // published regardless, so a failure here left the hook installed and
        // jumping into a page the processor will not execute -- an access
        // violation raised by the instrument, inside the first hooked call.
        // See [A failable call has its failure handled,
        // always](../../../../DESIGN-NOTES.md#a-failable-call-has-its-failure-handled-always).
        //
        // SAFETY: this module's own reservation, released exactly once, and
        // never published -- `TRAMPOLINES[index]` is still whatever it was.
        unsafe { VirtualFree(page, 0, MEM_RELEASE) };
        return Err(Refusal::TrampolineNotExecutable);
    }
    let previous = TRAMPOLINES[index].swap(page as usize, Ordering::Release);

    // The patch: `jmp qword ptr [rip+0]`, destination inline behind it.
    let mut patch = [0_u8; PATCH_LEN];
    patch[0] = 0xFF;
    patch[1] = 0x25;
    patch[6..].copy_from_slice(&hook.to_le_bytes());

    // Note what is deliberately NOT done here: opening the target page for
    // writing. `VirtualProtect` acts on whole pages, so two stubs that share
    // one page cannot each own its protection -- see `open_pages`.
    Ok(Prepared {
        entry,
        patch,
        index,
        trampoline: page,
        previous,
    })
}

/// Release a preparation the batch decided not to install.
///
/// [`prepare`] publishes its trampoline before the batch knows whether it will
/// commit, because the patch it builds points at that page. A batch that then
/// refuses used to return with the page still allocated and still published:
/// permanently reserved, never jumped to, and invisible -- bounded at one page
/// per hook, but a leak.
///
/// **The page is released only when this preparation found the slot empty.**
/// A non-zero `previous` means some earlier install already published a
/// trampoline for this hook, and an earlier install that succeeded also patched
/// the stub -- so the stub is live, it has been jumping through this slot, and a
/// thread may be inside the page right now or about to load it. Freeing there
/// would be a use-after-free on a running hook, which is a far worse defect than
/// the leak being fixed. That case restores the pointer it displaced and keeps
/// the page.
///
/// Only `install_by_label`, which is test-only, can reach the non-zero case:
/// `install_requested` runs `install_batch` once per process behind `DONE`, so
/// every slot it sees is zero. The branch is here because the function must be
/// correct for its callers rather than for the one that happens to exist.
///
/// With `previous` zero the stub was never patched -- a successful install
/// publishes the trampoline before it writes the stub, so an empty slot means no
/// write ever happened -- and the hook is therefore unreachable. Nothing can be
/// executing the page, and clearing the slot cannot be observed by a hook body.
#[cold]
fn discard_prepared(ready: &Prepared) {
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, VirtualFree};

    if ready.previous != 0 {
        TRAMPOLINES[ready.index].store(ready.previous, Ordering::Release);
        return;
    }
    TRAMPOLINES[ready.index].store(0, Ordering::Release);
    // SAFETY: this module's own reservation from `prepare`, released exactly
    // once -- the slot it was published in is cleared above and no stub was
    // ever patched to jump here, so nothing can reach it.
    unsafe { VirtualFree(ready.trampoline, 0, MEM_RELEASE) };
}

/// A page whose protection this batch changed, and what it was before.
pub(super) struct OpenedPage {
    base: *mut core::ffi::c_void,
    restore: u32,
}

/// Page size, asked of the system rather than assumed.
fn page_size() -> usize {
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

    // SAFETY: a live out-parameter of the right type.
    let mut info: SYSTEM_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: as above; no other preconditions.
    unsafe { GetSystemInfo(&mut info) };
    info.dwPageSize as usize
}

/// Make every page the batch will write to writable, once per page.
///
/// **Once per page is the whole point.** `VirtualProtect` reports the previous
/// protection of the *page*, not of the byte range asked about, so a per-stub
/// save is wrong as soon as two hooked stubs share a page -- and they do: on
/// this host `park`, `set-info` and `shutdown` land on one page. The second
/// stub would save `PAGE_EXECUTE_READWRITE`, because the first had just made it
/// so, and restoring in order would leave `ntdll` writable for the rest of the
/// process's life. An earlier revision of this module did exactly that; the
/// single-phase install it replaced did not, because it protected and restored
/// around each store in turn.
///
/// Returns `None` when any page could not be opened, having restored the ones
/// it already had. Partial success is not useful here: the batch is a
/// diagnostic that either installs or refuses.
pub(super) fn open_pages(entries: &[*mut u8]) -> Option<Vec<OpenedPage>> {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

    let size = page_size();
    let mut bases: Vec<usize> = Vec::new();
    for &entry in entries {
        // A fourteen-byte patch can straddle a boundary, so both pages count.
        let first = entry as usize & !(size - 1);
        let last = (entry as usize + PATCH_LEN - 1) & !(size - 1);
        let mut base = first;
        loop {
            if !bases.contains(&base) {
                bases.push(base);
            }
            if base == last {
                break;
            }
            base += size;
        }
    }

    let mut opened: Vec<OpenedPage> = Vec::with_capacity(bases.len());
    for base in bases {
        let base = base as *mut core::ffi::c_void;
        let mut restore = 0_u32;
        // SAFETY: a live code page in this process, asked about by page base.
        let ok = unsafe { VirtualProtect(base, size, PAGE_EXECUTE_READWRITE, &mut restore) };
        if ok == 0 {
            restore_pages(&opened);
            return None;
        }
        opened.push(OpenedPage { base, restore });
    }
    Some(opened)
}

/// Put every page's protection back, once per page.
pub(super) fn restore_pages(opened: &[OpenedPage]) {
    use windows_sys::Win32::System::Memory::VirtualProtect;

    let size = page_size();
    for page in opened {
        let mut ignored = 0_u32;
        // SAFETY: restoring the protection `open_pages` changed on this page.
        unsafe { VirtualProtect(page.base, size, page.restore, &mut ignored) };
    }
}

/// Store the patch. **This is the whole of what runs with threads suspended.**
///
/// A plain fourteen-byte store: no system call, no allocation, no lock, and
/// nothing that can block. That is the point of splitting the install in three
/// -- see [`install_batch`].
///
/// SAFETY: `prepared` must come from [`prepare`] and its entry must sit on a
/// page [`open_pages`] has made writable and not yet restored. `prepare`
/// deliberately does not open the page -- protection is per-page and two stubs
/// can share one -- so naming it as the opener, which this comment used to do,
/// stated a precondition no function establishes.
unsafe fn commit(prepared: &Prepared) {
    // SAFETY: `open_pages` made this page writable and `restore_pages` has not
    // run yet, for exactly this length.
    unsafe {
        std::ptr::copy_nonoverlapping(prepared.patch.as_ptr(), prepared.entry, PATCH_LEN);
    }
}

/// Flush the instruction cache for one patched stub, after the resume.
///
/// **After the resume deliberately.** It is a system call and so may not run
/// inside the window. That is sound on this target: the bytes were stored while
/// every other thread was stopped, so no thread can have observed a half-written
/// patch, and resuming a thread is a context switch, which is serialising -- a
/// resumed thread cannot go on executing a stale prefetch of the old stub.
///
/// Page protection is **not** restored here. That is per-page and belongs to
/// the batch; see [`restore_pages`].
///
/// SAFETY: `prepared` must come from [`prepare`] and have been committed.
unsafe fn finish(prepared: &Prepared) {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: no preconditions beyond a live process handle.
    unsafe { FlushInstructionCache(GetCurrentProcess(), prepared.entry.cast(), PATCH_LEN) };
}

/// Install every hook in `chosen`, suspending the other threads only for the
/// stores.
///
/// **Why this is three phases rather than one.** Patching live code requires
/// every other thread to be stopped, and a thread stopped while holding a
/// process-wide lock can never give it back -- so anything inside that window
/// which takes such a lock deadlocks the process with nothing able to run.
/// A single-phase install took three of them: the allocator, through
/// `ntdll_proc`'s owned name and the trampoline reservation; the **loader**,
/// through `GetModuleHandleA` and `GetProcAddress`; and the memory manager,
/// through `VirtualAlloc` and `VirtualProtect`.
///
/// An earlier version knew about the first of those -- the enumeration vectors
/// are reserved ahead of time for exactly that reason, and `sabotage.json`
/// carries a control recording it -- and missed the other two, which sat in
/// the function the window was wrapped around. Reserving a vector while calling
/// the loader three lines later is the shape of partial fix this repository
/// keeps producing: the rule was stated at the allocation it was noticed at
/// rather than at the window it belongs to.
///
/// So the window now holds [`commit`] and nothing else.
fn install_batch(chosen: &[usize]) -> Vec<(usize, Result<(), Refusal>)> {
    // Phase one, with everything still running: resolve, recognise, allocate
    // the trampolines.
    let prepared: Vec<(usize, Result<Prepared, Refusal>)> = chosen
        .iter()
        .map(|&index| (index, prepare(index)))
        .collect();

    // Phase one and a half, still running: open every page the stores will land
    // on, **once per page** rather than once per stub.
    let targets: Vec<*mut u8> = prepared
        .iter()
        .filter_map(|(_, outcome)| outcome.as_ref().ok().map(|ready| ready.entry))
        .collect();
    // Phases one-and-a-half through three, retried as a unit: open the pages,
    // patch with everything stopped, restore the pages. The open and the
    // restore are inside the attempt deliberately -- see `quiesce_once` -- so a
    // retry never leaves `ntdll` writable-and-executable across the wait.
    let mut refusal = Refusal::NotQuiesced;
    for attempt in 0..ATTEMPTS {
        let opened = if targets.is_empty() {
            Some(Vec::new())
        } else {
            open_pages(&targets)
        };
        let Some(opened) = opened else {
            // `open_pages` has already restored whatever it managed to open,
            // and nothing has been written. Not retried: a page this process
            // cannot make writable will not become writable a millisecond
            // later, which is unlike the terminating thread the retry is for.
            refusal = Refusal::NotWritable;
            break;
        };

        // The byte ranges the stores will land on, so the suspension can refuse
        // a thread parked inside one.
        let ranges: Vec<(usize, usize)> = prepared
            .iter()
            .filter_map(|(_, outcome)| outcome.as_ref().ok())
            .map(|ready| (ready.entry as usize, ready.entry as usize + PATCH_LEN))
            .collect();

        // Phase two, with every other thread stopped: stores only.
        let committed = quiesce_once(&ranges, || {
            for (_, outcome) in &prepared {
                if let Ok(ready) = outcome {
                    // SAFETY: prepared by `prepare`, on a page `open_pages`
                    // made writable and has not yet restored.
                    unsafe { commit(ready) };
                }
            }
        })
        .is_some();

        if !committed {
            // Nothing was written. Put the protection back before waiting, so
            // the pages are writable only for the attempt itself.
            restore_pages(&opened);
            if attempt + 1 < ATTEMPTS {
                std::thread::sleep(SETTLE);
            }
            continue;
        }

        // Phase three, running again: flush each patched stub, then put every
        // page's protection back exactly once.
        let outcomes: Vec<(usize, Result<(), Refusal>)> = prepared
            .into_iter()
            .map(|(index, outcome)| {
                let result = match outcome {
                    Ok(ready) => {
                        // SAFETY: prepared above and just committed.
                        unsafe { finish(&ready) };
                        Ok(())
                    }
                    Err(why) => Err(why),
                };
                (index, result)
            })
            .collect();
        restore_pages(&opened);
        return outcomes;
    }

    // Every attempt refused, so nothing was written and no page is still open.
    // Release the trampolines nothing will now jump through and refuse every
    // entry rather than reporting an install that did not happen.
    prepared
        .into_iter()
        .map(|(index, outcome)| {
            let outcome = match outcome {
                Ok(ready) => {
                    discard_prepared(&ready);
                    Err(refusal)
                }
                Err(why) => Err(why),
            };
            (index, outcome)
        })
        .collect()
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
    let (symbol, _) = HOOKS.iter().find(|(_, name)| *name == label)?;
    ntdll_proc(symbol).map(|found| found as *const u8)
}

/// An `ntdll` stub that this module can never patch, for the recogniser's
/// live canary.
///
/// The canary asks whether the recognised shape still describes a real entry
/// point on the running build. It has to read a stub whose bytes are the ones
/// Windows shipped, and [`stub_entry`] cannot promise that: every name it can
/// resolve is in [`HOOKS`], the patch is deliberately never removed, and a
/// sibling test installs one. Reading a patched stub turns the canary into an
/// assertion about the jump that was planted over it, which is both false and
/// the opposite of what it is for.
///
/// Measured rather than reasoned: with the hooking test running first in the
/// same process, the canary failed 25 of 25 runs. It passes in the full suite
/// only because the default thread count happens to order the two the other
/// way, which is luck that changes with the test count or the machine.
///
/// `NtQueryDefaultLocale` is the choice because it is absent from [`HOOKS`] --
/// asserted below, so adding it there is a loud failure rather than a silently
/// re-broken canary -- and because it is obscure enough that nothing else in
/// the process is likely to have hooked it either.
#[cfg(test)]
pub(crate) fn unhookable_stub_entry() -> Option<*const u8> {
    const SYMBOL: &str = "NtQueryDefaultLocale";
    assert!(
        !HOOKS.iter().any(|(symbol, _)| *symbol == SYMBOL),
        "{SYMBOL} is now in HOOKS, so this module can patch it and it is no \
         longer a canary for the shape Windows shipped -- pick another export \
         that is not in the table"
    );
    ntdll_proc(SYMBOL).map(|found| found as *const u8)
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
    // **This three-argument pointer is the `selftest` entry's arity guard**, and
    // is load-bearing rather than a convenience. Arity is a property of the
    // running operating system, so nothing in the build can establish it; what
    // can is calling the stub with the shape the table claims and requiring the
    // kernel's own answer to be consistent. If `NtQueryTimerResolution` ever
    // took a different number of parameters, this call would be malformed and
    // the companion assertions in
    // `a_hooked_stub_records_both_ends_and_still_performs_its_syscall` -- three
    // out-parameters written, ordered finest to coarsest -- would fail on the
    // next CI run.
    //
    // So do not relax those assertions into "the call returned": that is the
    // half that would still pass with the arity wrong.
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
/// Whether this module has already patched the stub behind a label.
///
/// A non-zero trampoline slot is the record of a successful install, and only
/// a successful install writes one.
///
/// Tests need this because the `.CRT$XCU` initialiser installs whatever
/// `WINDOWS_THREADPOOL_TRACE_HOOKS` asked for before any test runs. A test that
/// then installed unconditionally met its own installation: the recogniser sees
/// the patched entry, correctly refuses it as `NotAStub`, and the suite failed
/// in exactly the configuration that exercises the thing under test. Asking
/// first is what lets the assertions run against an installation this process
/// already has, without weakening the recogniser's rejection of a foreign one.
#[cfg(test)]
pub(crate) fn installed_by_label(label: &str) -> bool {
    HOOKS
        .iter()
        .position(|(_, name)| *name == label)
        .is_some_and(|index| TRAMPOLINES[index].load(Ordering::Acquire) != 0)
}

#[cfg(test)]
pub(crate) fn install_by_label(label: &str) -> Result<(), Refusal> {
    let Some(index) = HOOKS.iter().position(|(_, name)| *name == label) else {
        return Err(Refusal::NotFound);
    };
    install_batch(&[index])
        .into_iter()
        .next()
        .expect("one index in, one outcome out")
        .1
}

/// Install the hooks named by `WINDOWS_THREADPOOL_TRACE_HOOKS`, once.
///
/// The variable is a comma-separated list of the labels in the table above
/// (`park`, `release`, `ready`, `set-info`, `shutdown`), or `*` for all of
/// them. It is a separate variable from `WINDOWS_THREADPOOL_TRACE` on purpose:
/// tracing observes, this modifies another module's code, and the second must
/// never follow from the first.
///
/// **Setting it gives up hot-patching these stubs for the life of the
/// process**, and the patch is never removed. See [what enabling this costs
/// the process](self#what-enabling-this-costs-the-process-permanently).
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
    if SEALED.load(Ordering::SeqCst) {
        // Recorded rather than ignored: a refusal here means something called
        // this after the one-thread window closed, which is a defect in the
        // caller and must not look like "the hooks were not requested".
        record(TARGET, "refused-after-seal", 0, 0);
        return;
    }
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
    for (index, (_, label)) in HOOKS.iter().enumerate() {
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
    let outcomes = install_batch(&chosen);
    // Recorded after the resume. Nothing may take the trace lock while another
    // thread is stopped, possibly holding it.
    for (index, outcome) in outcomes {
        match outcome {
            Ok(()) => record(TARGET, "installed", index as u64, 0),
            Err(why) => record(TARGET, "refused", index as u64, why as u64),
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
/// Every worker factory in the process, as `(handle, maximum, total, waiting)`.
///
/// The maximum is included because it is the only field here that distinguishes
/// the factories: the default process pool's is large (768 on the machine this
/// was developed on), and a process holds at least one other factory with a
/// small one that this crate does not create. Without it a caller cannot tell
/// which row is which, and an earlier version of this function that guessed by
/// handle order produced a flaky test.
///
/// **Separate from `probe_factory` because that one picks the lowest handle**,
/// which is a heuristic for "the default pool" and not a selection. A process
/// holds more than one factory -- at least one this crate does not create -- and
/// the handle ordering between them is not guaranteed. A caller that needs to be
/// right about which factory it is looking at has to see them all.
pub(crate) fn probe_all_factories() -> Vec<crate::trace::WorkerFactorySnapshot> {
    // (handle, thread_maximum, total_worker_count, waiting_worker_count)
    let Some(raw) = ntdll_proc("NtQueryInformationWorkerFactory") else {
        return Vec::new();
    };
    // SAFETY: the name resolved in `ntdll` and this is its documented shape.
    let query: Query = unsafe { std::mem::transmute::<usize, Query>(raw) };

    let mut out = Vec::new();
    for handle in scan_for_factory(query) {
        let mut info = Basic::default();
        let mut returned = 0_u32;
        // SAFETY: `info` is a live, correctly sized buffer; the handle just
        // answered the same query during the scan.
        let status = unsafe {
            query(
                handle,
                WORKER_FACTORY_BASIC_INFORMATION,
                std::ptr::from_mut(&mut info).cast(),
                size_of::<Basic>() as u32,
                &mut returned,
            )
        };
        if status >= 0 {
            out.push(crate::trace::WorkerFactorySnapshot {
                handle,
                thread_maximum: info.thread_maximum,
                total_workers: info.total_worker_count,
                waiting_workers: info.waiting_worker_count,
            });
        }
    }
    out
}

/// Separate from [`counts`] because a guard needs values to assert on, while a
/// capture needs records. Returning them from `counts` would tempt a caller to
/// interpret a layout this module deliberately only records.
///
/// **Picks the lowest handle, which is a heuristic.** Use
/// [`probe_all_factories`] where being right about *which* factory matters.
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
    for (index, (_, label)) in HOOKS.iter().enumerate() {
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

/// `IoCompletionBasicInformation`, whose whole content is the queue depth.
const IO_COMPLETION_BASIC_INFORMATION: u32 = 0;

type QueryPort =
    unsafe extern "system" fn(usize, u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;

/// How much work is sitting on each completion port in this process, undelivered.
///
/// **This is the quantity that decides whether the pool creates a worker.** A
/// factory's create test approves when its completion port has work outstanding,
/// so a stalled pool whose port is non-empty was entitled to a thread and did not
/// get one -- while an empty port means the factory is behaving correctly on the
/// information it has and the fault lies earlier, in delivery. Those two readings
/// send the investigation in opposite directions, and nothing measured so far
/// separates them.
///
/// Found the same way as the factories, and for the same reason: the default
/// pool exposes no way to reach its port, but `NtQueryIoCompletion` succeeds
/// only on a completion port and returns an ordinary error on anything else. The
/// query reports the depth **without dequeuing**, so it cannot consume the very
/// packet whose presence is the question -- which matters here more than usual,
/// given how much of this investigation has been spent on instruments that
/// repaired the fault they were measuring.
///
/// Every port is recorded with its handle, never just the deepest or the first:
/// a process holding a private pool has more than one, and choosing between them
/// here would be interpreting rather than measuring.
pub(crate) fn port_depths() -> bool {
    scan_ports(|depth, handle| record(TARGET, "port-depth", depth as u64, handle as u64))
}

/// The scan itself, with the per-port action left to the caller.
///
/// Split out so the guard can collect what the capture records, rather than
/// testing a reimplementation of it. A probe that silently found nothing would
/// be indistinguishable from a genuinely empty port, and "empty" is one of the
/// two readings this instrument exists to separate -- so the thing under test
/// has to be this function and not a copy.
fn scan_ports(mut each: impl FnMut(u32, usize)) -> bool {
    /// Highest handle tried. Matches the factory scan's ceiling, for the same
    /// reason -- handles are allocated low and densely in a test process.
    const CEILING: usize = 4096;

    let Some(raw) = ntdll_proc("NtQueryIoCompletion") else {
        record(TARGET, "port-unavailable", 0, 0);
        return false;
    };
    // SAFETY: the name resolved in `ntdll` and this is its documented shape.
    let query: QueryPort = unsafe { std::mem::transmute::<usize, QueryPort>(raw) };

    let mut found = 0_u64;
    let mut candidate = 4_usize;
    while candidate <= CEILING {
        let mut depth = 0_u32;
        let mut returned = 0_u32;
        // SAFETY: `depth` is a live, correctly sized buffer for this class. An
        // unusable candidate is reported as an error status, not UB.
        let status = unsafe {
            query(
                candidate,
                IO_COMPLETION_BASIC_INFORMATION,
                std::ptr::from_mut(&mut depth).cast(),
                size_of::<u32>() as u32,
                &mut returned,
            )
        };
        if status >= 0 {
            found += 1;
            each(depth, candidate);
        }
        candidate += 4;
    }
    record(TARGET, "ports-seen", found, 0);
    found != 0
}

/// Every completion port the scan can see, as `(depth, handle)`.
///
/// For the guard, which needs values to assert on where a capture needs records.
#[cfg(test)]
pub(crate) fn probe_ports() -> Vec<(u32, usize)> {
    let mut seen = Vec::new();
    scan_ports(|depth, handle| seen.push((depth, handle)));
    seen
}

/// Record one factory's counters.
///
/// A record carries the handle in its second slot wherever that slot is free,
/// so a capture from a process holding more than one factory can be read apart
/// rather than averaged into nonsense. `counts()` reads *every* factory in the
/// process rather than guessing which is the default pool's, so without this a
/// reader could only attribute a row by its position in the capture.
///
/// The exceptions are the records that pair two counters the layout only makes
/// sense of together -- `counts-min-max`, `counts-creating`,
/// `counts-idle-timeout-ms`, `counts-start-routine`, `counts-stack`, and the
/// two status records -- which spend the second slot on the partner value.
/// Those are attributable by the `counts-total` that precedes them for the same
/// factory.
///
/// `counts-waiting` and `counts-pending` used to pass a literal zero here, and
/// they are the two this facility exists to read: a factory holding the packet,
/// with parked workers it does not consider available. They carry the handle.
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
        handle as u64,
    );
    record(
        TARGET,
        "counts-pending",
        info.pending_worker_count as u64,
        handle as u64,
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
    // M-T5.2. Whether the factory believes it is queued for a deferred thread
    // creation, and whether a deferred-create timer is armed.
    //
    // **These two were read and discarded for three days.** The layout has
    // carried them since the hooks were written; nothing emitted them, so every
    // capture so far has been silent about the one state that distinguishes the
    // two live readings of `M-T5.1`'s result. With work queued, no workers, and
    // no creation in progress or failed, a factory flagged as queued for
    // deferred creation is one whose creation was scheduled and never serviced;
    // a factory not so flagged is one nothing ever asked.
    //
    // Recorded as a pair because they are only meaningful together: a factory
    // queued with no timer armed is a different state from one with both.
    record(
        TARGET,
        "counts-deferred",
        info.queued_to_ex_worker as u64,
        info.timer_set as u64,
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
    // M-T5.7. Everything else the layout decodes.
    //
    // **Emitted without knowing which will matter, which is the point.** Three
    // hypotheses have now been refuted by fields that were already being read
    // and discarded, the last of them a pair that had looked no more promising
    // than these do. Guessing which field is interesting has a worse record here
    // than emitting all of them once and never guessing again. The cost is a
    // handful of records in a capture that only happens when something has
    // already gone wrong.
    record(
        TARGET,
        "counts-retry-timeout-ms",
        (info.retry_timeout / -10_000) as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-infinite-wait-goal",
        info.infinite_wait_goal as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-start-routine",
        info.start_routine as u64,
        info.start_parameter as u64,
    );
    record(
        TARGET,
        "counts-process-id",
        info.process_id as u64,
        handle as u64,
    );
    record(
        TARGET,
        "counts-stack",
        info.stack_reserve as u64,
        info.stack_commit as u64,
    );
    true
}

/// Post one packet to every completion port that already has work on it, and
/// report how many were poked.
///
/// **`M-T5.6`: does an ordinary arrival wake a stalled pool?** Two routes reach
/// a factory's create decision -- work outstanding on its completion port, and a
/// count of user-mode release requests. `NtReleaseWorkerFactoryWorker` arrives on
/// the second and is measured to recover this stall every time; queued work
/// arrives on the first and never does. Posting exercises the first route on
/// demand, inside a process that is already stalled, and either answer localises
/// the fault:
///
/// - a worker appears, and the arrival-to-factory link is intact, so the fault is
///   specific to how the victims' packets were inserted;
/// - no worker appears, and that link is severed for this port -- work can arrive
///   and nothing will ever notice.
///
/// `min_depth` selects which ports to poke, and both settings are needed:
///
/// - **1** for the stalled case, so only the port holding the stuck work is
///   poked -- that is the one whose pool is stalled, and poking an idle port
///   would answer a different question.
/// - **0** for the positive control, which pokes an idle port in a *healthy*
///   process. Without that control a null result is uninterpretable: "no worker
///   appeared" would look identical whether the link is severed or whether
///   posting a bare packet simply is not a stimulus that creates workers at all.
///
/// **This is destructive and is why it sits behind its own switch.** A posted
/// packet is not a real work item, so a worker that does appear may dispatch it
/// as garbage. That is acceptable only because the caller is a process which has
/// already failed and is about to panic; it must never run by default. The
/// measurement is the *worker count*, read before and after -- not whether
/// anything sensible ran.
pub(crate) fn poke_ports_with_work(min_depth: u32) -> u32 {
    use windows_sys::Win32::System::IO::PostQueuedCompletionStatus;

    let mut poked = 0_u32;
    let mut targets: Vec<(usize, u32)> = Vec::new();
    scan_ports(|depth, handle| {
        if depth >= min_depth {
            targets.push((handle, depth));
        }
    });
    for (handle, depth) in targets {
        record(TARGET, "port-poking", handle as u64, depth as u64);
        // SAFETY: `handle` answered `NtQueryIoCompletion`, so it is a completion
        // port. The packet carries no overlapped pointer.
        let ok = unsafe {
            PostQueuedCompletionStatus(handle as *mut core::ffi::c_void, 0, 0, std::ptr::null_mut())
        };
        record(TARGET, "port-poked", handle as u64, u64::from(ok != 0));
        if ok != 0 {
            poked += 1;
        }
    }
    record(TARGET, "ports-poked", u64::from(poked), 0);
    poked
}
