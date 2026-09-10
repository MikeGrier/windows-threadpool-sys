// Copyright (c) Mike Grier.

//! The `GetFullPathNameW` entry.
//!
//! Entry 9 of the audited catalogue, and the only one that takes neither a
//! handle nor produces one.
//!
//! # What it solves, and what it leaves standing
//!
//! This call **does not verify what it produces**: it will happily resolve a
//! path to something that does not exist, and it reports no error for one.
//!
//! That is the documented guarantee, and it is deliberately narrower than
//! "touches no filesystem", which earlier revisions of this doc claimed.
//! Microsoft specifies that the function does not verify that the resulting
//! path and file name are valid or that they name an existing file; it does not
//! specify that no I/O occurs.
//!
//! **And on one form it demonstrably does touch the filesystem.** Resolving a
//! drive-relative path for a drive other than the current one validates that
//! drive's recorded entry against the filesystem, and rewrites it when the
//! entry does not name an existing directory -- see "The drive-relative form
//! writes process state" below. So the narrow guarantee is the one to rely on
//! precisely because the broad one is false, not merely unproven. A caller
//! wanting existence must still open.
//!
//! It does **two** things, and keeping them apart is the whole reason this
//! entry exists:
//!
//! 1. It rewrites the string. `.` and `..` are collapsed, `/` becomes `\`, and
//!    trailing dots and spaces are trimmed. This part *is* lexical -- pure
//!    string work over the input, reading no process state. `C:\a\..\b` becomes
//!    `C:\b` whatever the current directory happens to be, and whether or not
//!    `C:\a` exists.
//! 2. It **roots** a path that is not fully qualified, using mutable process
//!    state -- and on one form it also *changes* that state. There are three
//!    such forms:
//!
//!    * A relative path like `rel.txt` is rooted at the *process current
//!      directory*.
//!    * A root-relative path like `\foo` takes only the *root* of that
//!      directory, giving `C:\foo` rather than its subtree -- and
//!      `\\server\share\foo` when the current directory is a UNC path, which
//!      is why this says root and not drive.
//!    * A drive-relative path like `C:foo` is rooted at the entry Windows
//!      keeps for that drive in the hidden `=C:` environment variables. For
//!      the *current* drive that entry is ignored and the process current
//!      directory wins.
//!
//! **A whole class of input short-circuits both.** When the input names a
//! legacy device and nothing else, it resolves into the device namespace and is
//! not rooted at all: `CON` becomes `\\.\CON`, not a file under the current
//! directory.
//!
//! "And nothing else" is doing real work, and is looser than it first looks.
//! These all reach a device: a bare name (`CON`), a trailing colon (`CON:`,
//! `CON::`), trailing dots or spaces (`CON.`, `CON `), and any casing
//! (`con`). These do not, and root normally: anything with more of a path
//! around it (`CON.txt`, `a\CON`, `.\CON`, `CON:x`), and `\CON`, which
//! becomes `Q:\CON` for a current directory on `Q:`.
//!
//! **Do not build a name filter from the list below.** The accepted names are
//! `CON`, `NUL`, `PRN`, `AUX`, `CONIN$`, `CONOUT$`, and `COM`/`LPT`
//! followed by a single digit -- where "digit" includes the *superscripts*
//! `COM^1`, `COM^2` and `COM^3` (U+00B9, U+00B2, U+00B3) as well as `1`-`9`.
//! An exhaustive scan of the character after `COM` accepts exactly
//! U+0031-U+0039, U+00B2, U+00B3 and U+00B9 on the tested build; `COM0` and
//! `COM10` are not devices. The superscripts are precisely the sort of member a
//! hand-written denylist omits, and this documentation asserted a list without
//! them until a review measured it -- so treat the set as *observed on one
//! build*, and prefer letting this call answer the question over reimplementing
//! its judgement.
//!
//! So the call is **not** lexical as a whole, and describing it that way -- as
//! an earlier revision of this doc did, in the sentence immediately before the
//! one describing the current directory it reads -- loses exactly the half that
//! matters here. A fully-qualified input resolves to the same output every
//! time; an input that is rooted resolves to different outputs in the same
//! process at different times, and pinning *that* is the property being bought.
//! (Not every unqualified input is rooted, which is the point of the device
//! short-circuit above: `CON` is unqualified and yet invariant.)
//!
//! So it solves exactly one problem -- the process current directory is shared
//! mutable state that any thread can change, so a relative path means something
//! different depending on *when* it is resolved. Performing this on the
//! submitting thread pins that meaning.
//!
//! # Why not a genuinely lexical canonicalizer
//!
//! Two exist: `PathCchCanonicalizeEx` and `PathAllocCanonicalize`. Both
//! canonicalize the string without rooting it.
//!
//! **They are the wrong call here, and the reason is a semantic difference, not
//! a cost one.** Resolving against the current directory *at submission* is what
//! this crate is buying. A lexical canonicalizer would leave a relative path
//! still relative, so its meaning would be decided on the worker thread at
//! execution time, against a current directory any thread may have changed in
//! between -- reintroducing exactly the race preparation exists to close. What
//! they omit is the part that is wanted.
//!
//! **No cost comparison is claimed, deliberately.** Nothing in this repository
//! benchmarks either alternative, Microsoft documents behaviour rather than
//! relative cost, and `PathAllocCanonicalize` allocates its own result -- so
//! "cheaper" would be a guess. It is also not needed: the decision rests on the
//! rooting semantics alone. Nor is either one reliably free of process state,
//! since `PATHCCH_ALLOW_LONG_PATHS` makes `PathCchCanonicalizeEx` consult the
//! process long-path setting unless the FORCE variant is used.
//!
//! Recorded so the next reader does not re-derive it. If this reasoning is ever
//! wrong -- for a consumer that genuinely wants a pure string operation and has
//! resolved relativity some other way -- the alternatives are named here.
//!
//! # The drive-relative form writes process state, and touches the filesystem
//!
//! Measured, and it overturns what four earlier revisions of this doc asserted.
//! Resolving `X:foo` for a drive that is **not** the current one does not
//! merely read the `=X:` entry:
//!
//! * An **accepted** entry is used **verbatim**, including a directory on a
//!   *different* drive. With `=X:` set to `C:\Windows`, `X:foo` resolves to
//!   `C:\Windows\foo`, so "that drive's own current directory" describes the
//!   convention the entry usually holds, not a guarantee about the result.
//!   Verbatim really means verbatim: `C:\Windows\` yields `C:\Windows\\foo`,
//!   with no normalisation at the join.
//! * Otherwise the entry is **written** to the drive root and that is used --
//!   created when absent, so this happens on a pristine host and not only on
//!   one carrying a stale entry. The write mutates the process environment
//!   block as a side effect of what reads like a pure query.
//!
//! **Acceptance needs both a shape and an existence check, and the observed
//! necessary conditions are worth listing because they are not guessable.** An
//! entry naming a directory that exists is still rejected unless it is already
//! in fully-qualified `X:\...` form: measured on one build, `C:/Windows/System32`,
//! `C:\Windows\System32\.`, `C:\Windows\System32\..\System32` and
//! `\\?\C:\Windows\System32` were each rejected while naming the same existing
//! directory that `C:\Windows\System32` was accepted for. An existing *file* and
//! a missing directory are rejected too, so existence is checked as well -- but
//! saying the gate is "a filesystem query rather than a syntax test", as a draft
//! of this doc did, states a mechanism the evidence contradicts. It is both, and
//! this list is a set of observations rather than a specification.
//!
//! For the current drive neither happens: the entry is not consulted and not
//! rewritten.
//!
//! This is why the "does not verify what it produces" guarantee above is worth
//! stating narrowly. The broad reading -- that the call touches no filesystem --
//! is not merely unproven, it is false here. Earlier revisions said the
//! opposite, reasoning that the current directory lives in the PEB and the
//! `=X:` variables in the environment block and that both are ordinary process
//! memory. The reasoning was sound and the conclusion wrong, which is the
//! standing hazard this crate keeps meeting: a mechanism argued from the data
//! sources rather than measured.
//!
//! # What a resolution costs
//!
//! The figure the repo's own instrument produces is a **bound, not this call's
//! cost**, and the difference matters. On x86_64 `probe-request-cost` measures
//! building an open request as a construct-and-drop cycle at roughly 210 ns and
//! cloning an already-resolved path at roughly 45 ns. The ~165 ns between them
//! is what recycling a resolved path recovers, and that is all it is.
//!
//! **That probe exercises [`crate::path::prepare`], not this module**, and the
//! two have different allocation shapes -- which is itself why the gap cannot be
//! read as this call's cost. `prepare` copies the input and then allocates a
//! `MAX_PATH` output buffer, so two allocations against the clone's one, and the
//! builder chain sits on top. [`ResolveFullPath`] takes its input already owned
//! and allocates one buffer per attempt instead. Either way the allocator work
//! is the crate's, not `GetFullPathNameW`'s, and attributing the gap to the call
//! -- as a draft of this doc did -- credits it with the work the same sentence
//! is busy excluding.
//!
//! Timed on its own -- input already marshalled, output buffer pre-allocated,
//! so no allocation is in the loop -- the call costs about **110 ns** on this
//! host, roughly two thirds of that gap. That measurement is a direct one taken
//! for this note and is *not* something the probe reports; no instrument in
//! this repository isolates the call, and the honest reading of
//! `probe-request-cost` alone is an upper bound.
//!
//! It does **not** solve the session-relative drive-letter hazard, and saying
//! so plainly matters more than the part it does solve. `GetFullPathNameW`
//! never expands a drive letter, and a drive letter is resolved against the
//! logon session of whatever token is in effect at open time. A path resolved
//! here and opened on a worker under a captured token from another logon
//! session can still name a different device. That hazard is open at the
//! workspace level; this entry inherits it and does not close it.
//!
//! A consumer that wants the *final*, filesystem-verified path of an object
//! wants [`crate::final_path`], which requires a handle and therefore an open.

use std::fmt;

use windows_sys::Win32::Storage::FileSystem::GetFullPathNameW;
use wtf_string::{Wtf16Str, Wtf16String};

use crate::outcome::{Win32Error, perform_nonzero};

/// How many times the buffer is grown before giving up.
///
/// As in [`crate::final_path`], one retry is the expected path; more means the
/// answer is changing under us.
const MAX_ATTEMPTS: usize = 8;

/// The buffer size the first attempt uses, in characters.
const FIRST_ATTEMPT_CHARS: usize = 260;

/// Why a full path could not be resolved.
///
/// This mirrors [`crate::final_path::FinalPathError`] deliberately: the two
/// entries share a retry shape, so they share a failure vocabulary. An earlier
/// revision returned a synthesized `ERROR_INSUFFICIENT_BUFFER` for the unstable
/// case, which left a caller unable to tell that apart from the same code
/// arriving from Windows, and made this entry the one place in the crate that
/// invented a code Win32 had not produced.
#[derive(Debug)]
#[non_exhaustive]
pub enum FullPathError {
    /// Windows refused the call, with the raw code unaltered.
    Win32(Win32Error),
    /// The required size kept changing, so the retry was abandoned.
    ///
    /// A path does not normally grow between two calls a microsecond apart, so
    /// this means something pathological rather than a transient. It is
    /// reported rather than looped on, because spinning here would hang the
    /// worker that a consumer moved this call onto in the first place.
    Unstable {
        /// How many attempts were made before giving up.
        attempts: usize,
    },
}

impl fmt::Display for FullPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Win32(error) => write!(f, "GetFullPathNameW: {error}"),
            Self::Unstable { attempts } => write!(
                f,
                "GetFullPathNameW: the required size changed on each of {attempts} attempts"
            ),
        }
    }
}

impl std::error::Error for FullPathError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Win32(error) => Some(error),
            Self::Unstable { .. } => None,
        }
    }
}

impl From<Win32Error> for FullPathError {
    fn from(error: Win32Error) -> Self {
        Self::Win32(error)
    }
}

/// An owned, marshalable parameter set for `GetFullPathNameW`.
///
/// # Example
///
/// ```
/// use windows_namespace_request_sys::full_path::ResolveFullPath;
/// use wtf_string::Wtf16String;
///
/// // `.` and `..` are collapsed as string work, with no component verified:
/// // this holds whether or not `C:\Windows\System32` exists.
/// let resolved = ResolveFullPath::new(Wtf16String::from(r"C:\Windows\System32\..\.\Temp"))
///     .perform()?
///     .to_string_lossy();
///
/// assert_eq!(resolved, r"C:\Windows\Temp");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// # Example: it does not check existence
///
/// ```
/// use windows_namespace_request_sys::full_path::ResolveFullPath;
/// use wtf_string::Wtf16String;
///
/// // A path to nothing resolves perfectly happily, because the call
/// // does not verify it. A consumer wanting a verified path wants an open
/// // plus GetFinalPathNameByHandleW instead.
/// let resolved = ResolveFullPath::new(Wtf16String::from(r"C:\no-such-directory\..\file.txt"))
///     .perform()?
///     .to_string_lossy();
///
/// assert_eq!(resolved, r"C:\file.txt");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug)]
#[must_use = "an unperformed request resolves nothing"]
pub struct ResolveFullPath {
    path: Wtf16String,
}

impl ResolveFullPath {
    /// Begins a request to resolve `path`.
    ///
    /// Takes a raw path rather than a [`crate::path::PreparedPath`], because
    /// preparation is what this call *performs*. Handing it an already-prepared
    /// path would be resolving twice.
    pub fn new(path: Wtf16String) -> Self {
        Self { path }
    }

    /// The path this request will resolve.
    #[must_use]
    pub fn path(&self) -> &Wtf16Str {
        &self.path
    }

    /// Performs the call on the calling thread, growing the buffer as needed.
    ///
    /// Resolution happens against the current directory of **whichever thread
    /// performs this**, which is the one thing a caller must keep in mind: a
    /// request built on a submitter and performed on a worker resolves against
    /// the process current directory as it stands at *performance* time.
    /// [`crate::path::prepare`] is the function for pinning that at
    /// construction.
    ///
    /// # Errors
    ///
    /// Returns [`FullPathError::Win32`] with the raw Win32 code, unaltered, or
    /// [`FullPathError::Unstable`] if the required size kept changing.
    pub fn perform(&self) -> Result<Wtf16String, FullPathError> {
        let mut capacity = FIRST_ATTEMPT_CHARS;

        for _ in 0..MAX_ATTEMPTS {
            let mut buffer = Wtf16String::with_capacity(capacity);
            let requested = u32::try_from(capacity).unwrap_or(u32::MAX);

            let written = perform_nonzero(|| {
                // SAFETY: the input has no interior NUL by Wtf16String's own
                // invariant for a terminated pointer, and the buffer is
                // writable for `requested` characters. The buffer's invariant
                // is restored below before it is observed.
                unsafe {
                    GetFullPathNameW(
                        self.path.as_terminated_ptr(),
                        requested,
                        buffer.as_mut_ptr(),
                        core::ptr::null_mut(),
                    )
                }
            })?;

            let written = written as usize;
            if written < capacity {
                // Success: `written` excludes the terminator.
                // SAFETY: exactly `written` content characters were written,
                // within the requested capacity.
                unsafe { buffer.set_len_from_ffi(written) };
                return Ok(buffer);
            }

            // Too small: `written` is the size required *including* the
            // terminator, and nothing usable was written.
            capacity = written;
        }

        // The required size kept changing across every attempt. Report it
        // rather than looping, for the reason final_path gives: spinning here
        // would hang the worker a consumer moved this call onto.
        //
        // Reported as its own variant rather than as a Win32 code. Windows has
        // none for "and it kept happening", and the nearest candidate --
        // `ERROR_INSUFFICIENT_BUFFER`, which each individual attempt really did
        // hit -- is one Win32 can also return on its own, so borrowing it would
        // leave a caller unable to tell the two apart.
        Err(FullPathError::Unstable {
            attempts: MAX_ATTEMPTS,
        })
    }
}

impl crate::request::Request for ResolveFullPath {
    type Error = FullPathError;
    type Output = Wtf16String;

    fn perform(&self) -> Result<Wtf16String, FullPathError> {
        Self::perform(self)
    }
}

#[cfg(test)]
mod tests;
