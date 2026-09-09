// Copyright (c) Mike Grier.

//! What does it cost to build a namespace request, against the queue that would
//! carry it?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # The decision this exists to inform
//!
//! The two-layer ring's submission queue was specified to carry **POD
//! descriptors with no allocation on push**. A deferred `CreateFileW` carries a
//! path, and a path is neither fixed-size nor POD, so that requirement and the
//! namespace plane's needs cannot both hold as written.
//!
//! `windows-namespace-request-sys` already solves the hard part: an `OpenFile`
//! is an *owned, `Send`* parameter set, built on one thread and performed
//! faithfully on another. So the queue can carry a request by value and the
//! lifetime hazard disappears. What remains is a cost question about **this
//! operation type**: how does building one compare with the doorbell that would
//! carry it (~165 ns as recorded on the Snapdragon X2 (ARM64) development
//! machine -- run `probe-doorbell-cost` on the host in front of you for a local
//! figure, which CI does in the same job)?
//!
//! # What this does not measure, stated because the number invites over-reading
//!
//! **This says nothing about whether the queue is efficient.** It measures the
//! construction cost of the queue's *heaviest* payload. Two distinctions the
//! result must not be stretched across:
//!
//!   - **Operation type.** A namespace open resolves a path through Win32 and
//!     may duplicate a handle. A registered-buffer read -- the hot path -- does
//!     neither: its descriptor is a slot index and an offset, and there the
//!     queue's own mechanics are the whole per-operation cost.
//!   - **Overhead against efficiency.** Throughput under contention, the ring's
//!     cache behaviour, batching amortization, and backpressure under load are
//!     what make a queue good or bad. A single uncontended construction time
//!     measures none of them.
//!
//! What it supports is a **comparison**, not a verdict: build cost against the
//! doorbell that would carry it. Which of the two is the larger half is a
//! question about one host, and this probe measures only one side of it --
//! read `probe-doorbell-cost`'s `set_reset_event` from the same run for the
//! other. The comparison inverts between machines: the Snapdragon X2 (ARM64)
//! development machine had the doorbell at roughly a third of a build, and an
//! x86_64 host measured during review had it at roughly two and a half times
//! one. A sentence naming a small half would therefore be wrong on one of them.
//! # Handle duplication is the part that is easy to under-count
//!
//! A request that carries a handle -- a template handle for an open, or the
//! subject of a query -- must **duplicate** it, because the submitting thread
//! may close its own copy the moment it returns. `CapturedHandle::capture` does
//! that with `DuplicateHandle`, which is a kernel transition, not a memory
//! copy. So "what does a request cost" is not only an allocation question, and
//! measuring only the path would understate it.
//!
//! # Preparing a path is a Win32 call, not an allocation
//!
//! This probe was written expecting `prepare` to be an allocation and a copy.
//! It is not: it calls **`GetFullPathNameW`** to resolve the path against the
//! process working directory, because [the namespace session] settled that the
//! path is resolved at submission -- the process CWD is mutable by any thread,
//! so even perfect remoting would be racy.
//!
//! That work reads **process state**: it resolves against the current
//! directory, and for a drive-relative path against the per-drive current
//! directory held in the `=C:` environment variables. So the measured remainder
//! is path resolution, not allocation -- and naming a *mechanism* for it has
//! now been got wrong twice. Calling it a *syscall cost* claimed a kernel
//! transition a timing loop cannot establish; calling it *lexical*, which
//! replaced it, claimed pure string work it equally is not. A genuinely lexical
//! canonicalizer is a different call (`PathCchCanonicalizeEx`) and is
//! deliberately not the one wanted here, because resolving against the CWD at
//! submission is the property being bought. What survives either way is the
//! part that matters: an allocator cannot remove it.
//!
//! The two schemes that might reduce it recover different halves. **Inline
//! storage** removes the allocation and copy, which is what
//! `clone_prepared_units` measures, and cannot touch the resolution at all.
//! **Recycling** a resolved path skips the resolution, paying the clone in
//! place of the whole build, so it recovers the difference between them. Naming
//! one figure for both -- as this did -- credits an allocator with work it
//! cannot remove. Knowing which half is which is the point of measuring both.
//! [the namespace session]: ../../../design-sessions/DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md
//!
//! Each timing is reported per operation. Absolute values are host-specific;
//! the **ratios against the doorbell and the atomic** are the finding.

use std::time::Instant;

use wtf_string::Wtf16String;

use windows_namespace_request_sys::{CapturedHandle, OpenFile, prepare};
use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, OPEN_EXISTING};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

/// Nanoseconds per operation for one timed loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// What was timed.
    pub label: &'static str,
    /// Iterations executed.
    pub iterations: u32,
    /// Nanoseconds per iteration.
    pub nanos_per_op: f64,
}

/// Every timing taken by [`measure`].
#[derive(Debug, Clone)]
pub struct Observation {
    /// Each timed loop, in the order run.
    pub timings: Vec<Timing>,
}

impl Observation {
    /// Look a timing up by label.
    #[must_use]
    pub fn get(&self, label: &str) -> Option<f64> {
        self.timings
            .iter()
            .find(|t| t.label == label)
            .map(|t| t.nanos_per_op)
    }
}

/// Time `body`, including the drop of whatever it returns.
///
/// **The drop is inside the timed region, and for the heap-owning values below
/// that is a construct-and-destroy cycle rather than a construction cost.**
/// `black_box` takes the value and it falls at the end of the statement, so
/// `prepare_*`, `build_open_request` and `clone_prepared_units` each include
/// freeing the `Wtf16String` they built. On a clone measured near 50 ns a free
/// is a visible share of the figure.
///
/// It is reported this way rather than restructured, and the reason is that the
/// obvious alternative is not more truthful. Retaining each value -- what the
/// captured-handle loop below does, for a reason that does not apply here --
/// would hold 100_000 live allocations, which measures an allocator that never
/// reuses a block instead of one that does. A queue holds a bounded number of
/// requests, so neither regime is the shipping one, and the honest course is to
/// say which one this is.
///
/// The captured-handle loop is different in kind and is genuinely restructured:
/// dropping a `CapturedHandle` calls `CloseHandle`, so leaving it in the timed
/// region reports two kernel transitions as one number. A free is not a
/// syscall.
fn time_loop<T>(label: &'static str, iterations: u32, mut body: impl FnMut() -> T) -> Timing {
    // Warm the path: the first pass pays for lazily resolved syscall stubs and
    // for the allocator's first touch of a fresh size class.
    for _ in 0..256 {
        std::hint::black_box(body());
    }
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(body());
    }
    let elapsed = start.elapsed();
    Timing {
        label,
        iterations,
        nanos_per_op: elapsed.as_nanos() as f64 / f64::from(iterations),
    }
}

/// Time request construction, path preparation, and handle duplication.
///
/// # Panics
///
/// Panics if the fixed test paths fail to prepare, which would mean
/// `prepare` rejects an ordinary absolute path and nothing here is meaningful.
#[must_use]
pub fn measure() -> Observation {
    const ITERATIONS: u32 = 100_000;
    const HANDLE_ITERATIONS: u32 = 50_000;

    // Resolved, not assumed. Windows is not always on `C:` -- a valid
    // installation can sit on any volume -- and hard-coding it made this probe
    // panic on such a machine rather than measure it. The same path is used for
    // the prepared request and the real open below, so the two stay consistent.
    // Wide the whole way, with no UTF-8 in the middle. `Wtf16String` exists
    // precisely to carry what Windows hands back, and routing a Windows path
    // through `str` gives up that property twice over: `to_str` panics on a
    // path that is not valid UTF-8, and the `from_utf16_lossy` this once used
    // inside `system_directory` silently replaced any unpaired surrogate before
    // it ever got here. Neither is reachable on a normal install, and neither
    // has any business being on the path from a Win32 call to a WTF-16 string.
    let system_dll = system_directory().join("kernel32.dll");
    let short = Wtf16String::from_os_str(system_dll.as_os_str());
    // Synthetic, and hard-coded on purpose -- the opposite requirement to the
    // path above, which is why the two do not match and must not be made to.
    // `short` names a file that is really opened, so it has to exist and is
    // resolved. This one is only ever normalized, so it must NOT need to exist:
    // a fixed 24-component path keeps the length identical on every host, and a
    // length that varied with the local system directory would make the figure
    // incomparable between the machines the report asks a reader to compare.
    //
    // The `C:` is safe for the same reason the probe's own conclusion is: a
    // fully-qualified path is normalized without consulting a device, so no
    // volume is needed behind the letter. Two review passes read this as the
    // portability bug fixed above, so it is now measured rather than argued --
    // see `preparing_a_path_needs_no_volume_behind_its_drive_letter`.
    let long_text = format!(r"C:\{}\file.txt", vec!["directory"; 24].join("\\"));
    let long = Wtf16String::from(long_text.as_str());

    let mut timings = Vec::new();

    // The allocation and normalization a path costs, at two lengths, because
    // the common case and the worst case allocate differently.
    timings.push(time_loop("prepare_short_path", ITERATIONS, || {
        prepare(&short).expect("an absolute path prepares")
    }));
    timings.push(time_loop("prepare_long_path", ITERATIONS, || {
        prepare(&long).expect("an absolute path prepares")
    }));

    // A whole request, which is a prepared path plus the builder chain. This is
    // what the queue would actually carry.
    timings.push(time_loop("build_open_request", ITERATIONS, || {
        let path = prepare(&short).expect("an absolute path prepares");
        OpenFile::new(path)
            .with_desired_access(GENERIC_READ)
            .with_share_mode(FILE_SHARE_READ)
            .with_creation_disposition(OPEN_EXISTING)
    }));

    // Cloning the prepared path alone, which is what a request-recycling scheme
    // would avoid paying.
    let prepared_units = prepare(&short)
        .expect("an absolute path prepares")
        .into_wtf16();
    timings.push(time_loop("clone_prepared_units", ITERATIONS, || {
        prepared_units.clone()
    }));

    // The kernel transition a captured handle costs. Measured against a handle
    // this process already owns, so nothing here depends on the filesystem.
    //
    // Capture and close are timed SEPARATELY, and that separation is the whole
    // point. `time_loop` black-boxes its closure's return value and drops it at
    // the end of the statement, so a loop that captures and returns a
    // `CapturedHandle` -- which owns an `OwnedHandle` -- also calls
    // `CloseHandle` inside the timed region. That is two kernel transitions
    // reported as one number, and the report reads that number as the cost of
    // duplication alone, so it overstated it by however much a close costs.
    //
    // Retaining every duplicate in a pre-sized `Vec` keeps the close out of the
    // capture loop, and timing the drop of that same `Vec` recovers the close as
    // its own figure rather than discarding it. The `push` is a pointer bump
    // into reserved capacity, which is not free but is nowhere near a syscall.
    let file = std::fs::File::open(&system_dll).expect("kernel32.dll is readable");
    let borrowed = std::os::windows::io::AsHandle::as_handle(&file);

    // Warmed the same way `time_loop` warms, and for the same reason: the first
    // pass pays for lazily resolved syscall stubs and the allocator's first
    // touch of a fresh size class.
    for _ in 0..256 {
        let _ =
            std::hint::black_box(CapturedHandle::capture(borrowed).expect("duplicating a handle"));
    }

    let mut captured = Vec::with_capacity(HANDLE_ITERATIONS as usize);
    let start = Instant::now();
    for _ in 0..HANDLE_ITERATIONS {
        captured.push(CapturedHandle::capture(borrowed).expect("duplicating an owned handle"));
    }
    let capture_elapsed = start.elapsed();
    timings.push(Timing {
        label: "capture_handle",
        iterations: HANDLE_ITERATIONS,
        nanos_per_op: capture_elapsed.as_nanos() as f64 / f64::from(HANDLE_ITERATIONS),
    });

    let start = Instant::now();
    drop(captured);
    let close_elapsed = start.elapsed();
    timings.push(Timing {
        label: "close_handle",
        iterations: HANDLE_ITERATIONS,
        nanos_per_op: close_elapsed.as_nanos() as f64 / f64::from(HANDLE_ITERATIONS),
    });

    Observation { timings }
}

/// Where Windows is actually installed, rather than where it usually is.
///
/// A short buffer is retried at the size the call asks for, so a long system
/// directory is read rather than guessed at.
///
/// # Panics
///
/// Panics if `GetSystemDirectoryW` will not report a directory.
///
/// This used to fall back to `C:\Windows\System32`, under a doc comment
/// claiming the fallback kept the failure visible. It did the opposite: it
/// substituted a guess for an answer the system declined to give, and the probe
/// then measured whatever happened to be at the guessed path -- on a non-`C:`
/// install, something that is not there at all. That is the same defect as
/// discarding a status: a plausible result standing in for one that was never
/// obtained. A probe that cannot locate the file it is timing has nothing to
/// say, and says so here rather than several frames later.
fn system_directory() -> std::path::PathBuf {
    // The common case, off the stack.
    let mut buffer = [0_u16; 260];
    // SAFETY: writes at most `buffer.len()` units into a buffer of that size.
    let written = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;

    assert!(
        written != 0,
        "GetSystemDirectoryW failed: {}",
        std::io::Error::last_os_error()
    );

    // `>=`, not `>`. On success the count excludes the terminator, so it can
    // reach at most `buffer.len() - 1`; when the buffer is too small it is the
    // required size *including* the terminator, so it is at least
    // `buffer.len() + 1`. Exactly `buffer.len()` is therefore unreachable from
    // either branch -- and treating it as too-small costs one wasted retry
    // while removing the need for the next reader to redo that analysis before
    // trusting a possibly-unterminated buffer.
    if written >= buffer.len() {
        // `written` is the required size including the terminator, so a buffer
        // of exactly that length is enough and the retry cannot ask again.
        let mut heap = vec![0_u16; written];
        // SAFETY: writes at most `heap.len()` units into a buffer of that size.
        let retried = unsafe { GetSystemDirectoryW(heap.as_mut_ptr(), heap.len() as u32) } as usize;
        assert!(
            retried != 0 && retried < heap.len(),
            "GetSystemDirectoryW failed at the size it asked for ({written}): {}",
            std::io::Error::last_os_error()
        );
        return std::path::PathBuf::from(os_string(&heap[..retried]));
    }

    std::path::PathBuf::from(os_string(&buffer[..written]))
}

/// Losslessly, because a Windows path is UTF-16 and not necessarily Unicode.
fn os_string(units: &[u16]) -> std::ffi::OsString {
    std::os::windows::ffi::OsStringExt::from_wide(units)
}
