// Copyright (c) Mike Grier.

//! Which `SEM_` bits are settable per thread, and how the thread error mode
//! relates to the process error mode.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use: that scope is what lets one do things a
//! shipping component must not. Do not call them from production code, and do
//! not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! This decides what a thread-ambient-state crate can offer as a declarable
//! aspect: a bit that cannot be set per thread cannot be offered at all, and a
//! bit that can be *observed* but not *set* would be a value capture could
//! produce and application could not install.

use windows_sys::Win32::System::Diagnostics::Debug::{
    GetErrorMode, GetThreadErrorMode, SetErrorMode, SetThreadErrorMode,
};

use windows_sys::Win32::Foundation::GetLastError;

/// The `SEM_*` values, named rather than written as bare literals at each use.
///
/// Changing any value here is a breaking change: they are Win32 ABI constants,
/// not a numbering this crate is free to choose.
pub mod bits {
    /// Suppress the critical-error handler dialog.
    pub const FAIL_CRITICAL_ERRORS: u32 = 0x0001;
    /// Suppress the general-protection-fault error box.
    pub const NO_GP_FAULT_ERROR_BOX: u32 = 0x0002;
    /// Suppress alignment-fault exceptions. Process-scoped and sticky.
    pub const NO_ALIGNMENT_FAULT_EXCEPT: u32 = 0x0004;
    /// Suppress the file-open error box.
    pub const NO_OPEN_FILE_ERROR_BOX: u32 = 0x8000;
}

/// What happened when one bit was set on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitOutcome {
    /// The bit that was attempted.
    pub bit: u32,
    /// Whether `SetThreadErrorMode` reported success.
    pub set_ok: bool,
    /// `GetLastError` when it did not.
    pub last_error: u32,
    /// What `GetThreadErrorMode` reported afterwards.
    pub read_back: u32,
}

impl BitOutcome {
    /// The bit was accepted **and** is actually in force.
    ///
    /// Both halves matter. A bit that is accepted but not installed would let a
    /// caller believe it applied a value it did not apply, which is a worse
    /// failure than an outright rejection.
    #[must_use]
    pub fn is_settable(self) -> bool {
        self.set_ok && self.read_back & self.bit == self.bit
    }

    /// The bit was accepted and then silently dropped.
    #[must_use]
    pub fn is_silently_dropped(self) -> bool {
        self.set_ok && self.read_back & self.bit != self.bit
    }
}

/// Holds the thread error mode at a known value, restoring the entry value on
/// drop.
///
/// # Why a probe needs this
///
/// The thread error mode is ambient state shared by every probe that runs on
/// this thread, and under `--test-threads=1` libtest runs tests inline on one
/// thread. A probe that reads the mode back therefore cannot tell what *this*
/// call did from what the thread was already carrying, unless the baseline is
/// forced rather than assumed.
///
/// Restoring on drop rather than after the measurement keeps the mode from
/// leaking when a probe panics between the two.
struct ThreadErrorMode {
    previous: u32,
}

impl ThreadErrorMode {
    /// Forces `mode`, and checks that it took.
    ///
    /// # Panics
    ///
    /// Panics if the mode cannot be set, or if reading it back does not return
    /// what was just installed -- a probe that could not establish its own
    /// baseline must stop rather than report against an unknown one.
    fn force(mode: u32) -> Self {
        let mut previous = 0u32;
        // SAFETY: `previous` is a valid writable destination; the call has no
        // other preconditions.
        let ok = unsafe { SetThreadErrorMode(mode, &raw mut previous) };
        assert_ne!(
            ok,
            0,
            "establish the thread error-mode baseline {mode:#06x} (last error {})",
            // SAFETY: no preconditions.
            unsafe { GetLastError() }
        );

        // SAFETY: no preconditions.
        let observed = unsafe { GetThreadErrorMode() };
        assert_eq!(
            observed, mode,
            "the forced baseline must actually be in effect, or the read-back \
             below measures something else"
        );

        Self { previous }
    }
}

impl Drop for ThreadErrorMode {
    fn drop(&mut self) {
        let mut ignored = 0u32;
        // SAFETY: restoring the exact value `force` saved.
        let restored = unsafe { SetThreadErrorMode(self.previous, &raw mut ignored) };
        assert_ne!(
            restored, 0,
            "restore the thread error mode; leaving it changed would contaminate \
             every later probe on this thread"
        );
    }
}

/// Set `mode` on this thread, read the result back, and restore the entry value.
///
/// A failed restore is a probe failure, not something to ignore: the mode is
/// ambient and shared, so a thread left carrying a probe's mode makes every
/// later probe on it report against a baseline nobody established.
fn with_thread_mode(mode: u32) -> (bool, u32, u32) {
    let mut previous = 0u32;
    // SAFETY: `previous` is a valid writable destination; the call has no other
    // preconditions.
    let ok = unsafe { SetThreadErrorMode(mode, &raw mut previous) };
    let last_error = if ok == 0 {
        // SAFETY: no preconditions.
        unsafe { GetLastError() }
    } else {
        0
    };
    // SAFETY: no preconditions.
    let read_back = unsafe { GetThreadErrorMode() };
    if ok != 0 {
        let mut ignored = 0u32;
        // SAFETY: restoring the exact value the call above saved.
        let restored = unsafe { SetThreadErrorMode(previous, &raw mut ignored) };
        assert_ne!(
            restored, 0,
            "restore the thread error mode after probing {mode:#06x}; leaving it \
             changed would contaminate every later probe on this thread"
        );
    }
    (ok != 0, last_error, read_back)
}

/// Attempt one bit on its own and report what the platform did with it.
#[must_use]
pub fn probe_bit(bit: u32) -> BitOutcome {
    let (set_ok, last_error, read_back) = with_thread_mode(bit);
    BitOutcome {
        bit,
        set_ok,
        last_error,
        read_back,
    }
}

/// Every bit this platform accepts per thread, as a mask.
#[must_use]
pub fn settable_bits() -> u32 {
    [
        bits::FAIL_CRITICAL_ERRORS,
        bits::NO_GP_FAULT_ERROR_BOX,
        bits::NO_ALIGNMENT_FAULT_EXCEPT,
        bits::NO_OPEN_FILE_ERROR_BOX,
    ]
    .into_iter()
    .filter(|bit| probe_bit(*bit).is_settable())
    .fold(0, |mask, bit| mask | bit)
}

/// Does one invalid bit cost the caller the whole call, or only that bit?
///
/// Returns `(installed_nothing, read_back)`. This is the finding that decides
/// whether a declarable type may merely *validate* an invalid bit or must be
/// unable to represent it: if an invalid bit fails the whole call, a caller
/// combining it with valid bits silently loses the entire change.
#[must_use]
pub fn combined_invalid_installs_nothing() -> (bool, u32) {
    let valid = bits::FAIL_CRITICAL_ERRORS | bits::NO_GP_FAULT_ERROR_BOX;
    let combined = valid | bits::NO_ALIGNMENT_FAULT_EXCEPT;

    // The whole finding is read out of `read_back`, and the call under test is
    // expected to *fail* -- so `read_back` is whatever the thread was already
    // carrying. Without a forced baseline, a thread that happened to hold
    // either valid bit would make this report that the call installed them,
    // which is the opposite of what happened. Forcing zero makes any valid bit
    // in the read-back attributable to this call and nothing else.
    let _baseline = ThreadErrorMode::force(0);

    let (_, _, read_back) = with_thread_mode(combined);
    (read_back & valid == 0, read_back)
}

/// What the thread mode reported while the process mode held `bit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessVersusThread {
    /// The bit installed at process scope.
    pub process_bit: u32,
    /// The process mode actually observed.
    pub process_mode: u32,
    /// The thread mode observed at the same moment.
    pub thread_mode: u32,
}

impl ProcessVersusThread {
    /// The process bit did **not** show through the thread mode.
    #[must_use]
    pub fn is_independent(self) -> bool {
        self.process_mode & self.process_bit == self.process_bit
            && self.thread_mode & self.process_bit == 0
    }
}

/// Is the thread error mode independent storage, or a view of the process mode?
///
/// This matters because capture reads the thread mode back. If a process bit
/// showed through, capture could observe a value a declarable type cannot hold
/// -- and a type unable to represent a state the platform can produce is a bug
/// rather than a safeguard.
///
/// Deliberately probed with a **reversible** bit. The obvious candidate,
/// `NO_ALIGNMENT_FAULT_EXCEPT`, is sticky at process scope, so using it here
/// would permanently mutate whatever process called this -- see
/// [`alignment_bit_is_sticky_at_process_scope`], which is binary-only for
/// exactly that reason.
///
/// # This one mutates process-wide state, and is not concurrency-safe
///
/// `SetErrorMode` is **process-scoped**: for the length of this call the whole
/// process carries a mode it did not ask for. That is what the module banner's
/// "things a shipping component must not" means concretely here, and it is the
/// only way to answer the question -- to learn whether the thread mode is a view
/// of the process mode, the process mode has to move.
///
/// Two overlapping calls can interleave their save and restore so that the entry
/// mode is lost and the probe's bit is left installed permanently. Hardening that
/// is a small change and is knowingly declined, because a hardened version would
/// look fit for a setting it must never enter; see `DESIGN-NOTES.md`.
#[must_use]
pub fn thread_mode_independent_of_process() -> ProcessVersusThread {
    let bit = bits::FAIL_CRITICAL_ERRORS;
    // SAFETY: none of these calls has preconditions.
    let previous = unsafe { SetErrorMode(bit) };
    let process_mode = unsafe { GetErrorMode() };
    let thread_mode = unsafe { GetThreadErrorMode() };
    unsafe { SetErrorMode(previous) };
    ProcessVersusThread {
        process_bit: bit,
        process_mode,
        thread_mode,
    }
}

/// Confirm that the alignment bit cannot be cleared once set at process scope.
///
/// **Binary only, and irreversible.** Calling this permanently sets
/// `SEM_NOALIGNMENTFAULTEXCEPT` on the calling process; the restore afterwards
/// is ignored by Windows, which is the very thing being demonstrated. It is not
/// a test because a test may not leave the process it ran in altered.
///
/// Returns `(mode_before, mode_after_restore_attempt)`.
#[must_use]
pub fn alignment_bit_is_sticky_at_process_scope() -> (u32, u32) {
    // SAFETY: none of these calls has preconditions.
    let before = unsafe { GetErrorMode() };
    unsafe { SetErrorMode(before | bits::NO_ALIGNMENT_FAULT_EXCEPT) };
    unsafe { SetErrorMode(before) };
    let after = unsafe { GetErrorMode() };
    (before, after)
}
