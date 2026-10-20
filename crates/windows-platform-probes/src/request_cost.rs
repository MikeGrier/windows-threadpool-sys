// Copyright (c) Mike Grier.

//! What does it cost to build a namespace request, against the queue that
//! would carry it?
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
//!   - **Operation type.** A namespace open carries a path and may carry a
//!     handle. A registered-buffer read -- the hot path -- does neither: its
//!     descriptor is a slot index and an offset, and there the queue's own
//!     mechanics are the whole per-operation cost.
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
//! one.
//!
//! # What building a request does
//!
//! An `OpenFile` is handed an owned path and keeps it as given. When the path
//! is a plain relative one -- the rules are
//! [D-19](../../windows-namespace-request-sys/DESIGN-NOTES.md#d-19) -- the
//! request also reads the process current directory when it is built and holds
//! the path grafted onto it, by concatenation, so it names the same place when
//! it is performed on another thread. Any other path, an absolute one among
//! them, is kept as given and no directory is read. Nothing is resolved,
//! length-checked or rewritten either way.
//!
//! So there are two build costs worth separating, and this probe times both:
//! a path **kept as given** (an absolute one, at two lengths) and a path
//! **grafted** (a relative one). The string a build is handed is cloned inside
//! each timed build, because a caller must supply an owned path; `clone_path_string`
//! is that clone on its own, so a reader can see how much of a build it is.
//!
//! # A handle is shared with a request, not duplicated for it
//!
//! A request that carries a handle -- a template handle for an open, or the
//! subject of a query -- holds a `SharedHandle` (`D-32` in the request crate's
//! design notes). Giving it one is a clone of a handle the caller already
//! shares, which counts one more holder; the handle is closed when the last
//! holder drops it, so the submitting thread may drop its own copy the moment it
//! returns. `share_handle` times that clone and its drop, and
//! `build_request_with_handle` times a build that also carries a template, to be
//! read beside `build_request_kept`, the same build without one.
//!
//! A caller that holds only a **borrowed** handle has no shared one to give, and
//! making one is a `DuplicateHandle`: a kernel transition, paid by that caller
//! **outside** the request crate before any request is built.
//! `duplicate_borrowed_handle` times it, and `close_duplicate` the close of each
//! duplicate, timed apart. Neither is a cost of building a request.
//!
//! # Performing is timed too, against a real file
//!
//! `perform_open` and `build_and_perform_open` open `kernel32.dll` for reading,
//! so the cost of building can be read beside the cost of the call it carries.
//! The file is opened and closed, never read.
//!
//! Each timing is reported per operation. Absolute values are host-specific;
//! the **ratios against the doorbell and the atomic** are the finding.

use std::time::Instant;

use wtf_string::Wtf16String;

use std::os::windows::io::{AsHandle, OwnedHandle};

use win_shared_os_owned_handle::SharedHandle;
use windows_namespace_request_sys::OpenFile;
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

/// The machine-readable name for a timing's label.
///
/// **The one place a figure's two names are related.** Both renderings walk
/// [`Observation::timings`] and this decides only what the machine-readable one
/// calls each entry, so the prose row and the NDJSON field cannot disagree about
/// a value or exist without each other.
///
/// Written for the same reason as `doorbell_cost::json_key`: the prose iterated
/// what was measured while the NDJSON named each field by hand, making them two
/// independent restatements of one measurement. The M2.4 matrix found that class
/// unchecked here, and deriving both from one source makes the disagreement
/// unrepresentable rather than merely detectable.
///
/// The suffix says what the figure is: `_cycle_ns` is a construct-and-drop
/// cycle, and `_ns` is a single operation timed apart from its drop.
///
/// # Panics
///
/// Panics on a label it does not know, so a timing added to `measure` without a
/// name here fails at the render rather than silently missing from the
/// machine-readable line -- the omission a fleet survey would never notice.
#[must_use]
pub fn json_key(label: &str) -> &'static str {
    match label {
        "clone_path_string" => "clone_path_string_cycle_ns",
        "build_request_kept" => "build_request_kept_cycle_ns",
        "build_request_kept_long" => "build_request_kept_long_cycle_ns",
        "build_request_grafted" => "build_request_grafted_cycle_ns",
        "clone_request_kept" => "clone_request_kept_cycle_ns",
        "clone_request_grafted" => "clone_request_grafted_cycle_ns",
        "perform_open" => "perform_open_cycle_ns",
        "build_and_perform_open" => "build_and_perform_open_cycle_ns",
        "share_handle" => "share_handle_cycle_ns",
        "build_request_with_handle" => "build_request_with_handle_cycle_ns",
        "duplicate_borrowed_handle" => "duplicate_borrowed_handle_ns",
        "close_duplicate" => "close_duplicate_ns",
        other => panic!(
            "`{other}` is measured but has no machine-readable name; add it to \
             `json_key` so it reaches the NDJSON line too"
        ),
    }
}

/// Every label [`measure`] records, in the order the report renders them.
///
/// All of them are unconditional -- unlike `doorbell_cost`, nothing here is
/// gated on a platform capability -- so this list needs no presence marker. It
/// exists for the direction a renderer that walks [`Observation::timings`]
/// cannot check: iterating what was measured cannot notice that something
/// *stopped* being measured, so a dropped timing would quietly shrink the
/// NDJSON schema. `main` caught that with a per-field panic; this is the same
/// guard, stated once.
pub const EVERY_LABEL: [&str; 12] = [
    "clone_path_string",
    "build_request_kept",
    "build_request_kept_long",
    "build_request_grafted",
    "clone_request_kept",
    "clone_request_grafted",
    "perform_open",
    "build_and_perform_open",
    "share_handle",
    "build_request_with_handle",
    "duplicate_borrowed_handle",
    "close_duplicate",
];

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
/// `black_box` takes the value and it falls at the end of the statement, so the
/// string, build and clone rows each include freeing what they built. On a
/// clone measured near 50 ns a free is a visible share of the figure. For the
/// two `perform` rows the drop is the `CloseHandle` of the handle the open
/// returned, so those are an open-and-close cycle.
///
/// It is reported this way rather than restructured, and the reason is that the
/// obvious alternative is not more truthful. Retaining each value -- what the
/// duplicated-handle loop below does, for a reason that does not apply here --
/// would hold 100_000 live allocations, which measures an allocator that never
/// reuses a block instead of one that does. A queue holds a bounded number of
/// requests, so neither regime is the shipping one, and the honest course is to
/// say which one this is.
///
/// The duplicated-handle loop is different in kind and is genuinely restructured:
/// dropping the `OwnedHandle` a duplication returns calls `CloseHandle`, so
/// leaving it in the timed region reports two kernel transitions as one number. A
/// free is not a syscall. The `share_handle` row needs no such care: dropping a
/// clone of a `SharedHandle` that another holder keeps closes nothing.
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

/// A request to open `path` for reading, the shape the performed rows use.
///
/// Takes the path by reference and clones it, because the request owns its
/// path and a caller has to supply one: that clone is part of every build this
/// probe times, and `clone_path_string` is it alone.
fn open_request(path: &Wtf16String) -> OpenFile {
    OpenFile::new(path.clone())
        .expect("the fixed sample paths have no NUL")
        .with_desired_access(GENERIC_READ)
        .with_share_mode(FILE_SHARE_READ)
        .with_creation_disposition(OPEN_EXISTING)
}

/// Time request construction, request cloning, performing an open, sharing a
/// handle with a request, and duplicating a borrowed handle outside one.
///
/// # Panics
///
/// Panics if a fixed sample path cannot build a request, or `kernel32.dll`
/// cannot be opened, which would mean nothing here is meaningful.
#[must_use]
pub fn measure() -> Observation {
    const ITERATIONS: u32 = 100_000;
    // An open and a close are kernel calls, so these are timed over fewer
    // iterations than the loops that only allocate.
    const PERFORM_ITERATIONS: u32 = 20_000;
    const HANDLE_ITERATIONS: u32 = 50_000;

    // Resolved, not assumed. Windows is not always on `C:` -- a valid
    // installation can sit on any volume -- and hard-coding it made this probe
    // panic on such a machine rather than measure it. The same file is built
    // into a request and really opened below, so the two stay consistent.
    // Wide the whole way, with no UTF-8 in the middle. `Wtf16String` exists
    // precisely to carry what Windows hands back, and routing a Windows path
    // through `str` gives up that property twice over: `to_str` panics on a
    // path that is not valid UTF-8, and the `from_utf16_lossy` this once used
    // inside `system_directory` silently replaced any unpaired surrogate before
    // it ever got here.
    let system_dll = system_directory().join("kernel32.dll");
    let kept = Wtf16String::from_os_str(system_dll.as_os_str());

    // The same file as a relative name, so a build grafts it onto the current
    // directory. It is only ever built, never performed: the file need not be in
    // the current directory.
    let relative = Wtf16String::from("kernel32.dll");

    // Synthetic, and hard-coded on purpose -- the opposite requirement to the
    // path above, which is why the two do not match and must not be made to.
    // `kept` names a file that is really opened, so it has to exist. This one is
    // only ever built into a request, so it must NOT need to exist: a fixed
    // 24-component path keeps the length identical on every host, and a length
    // that varied with the local system directory would make the figure
    // incomparable between the machines the report asks a reader to compare.
    //
    // The `C:` is safe, and what makes it safe is an OUTCOME, not a mechanism:
    // building a request succeeds with no volume behind the drive letter.
    // Whether a device is consulted is neither established here nor needed. The
    // outcome is measured rather than argued -- see
    // `building_a_request_needs_no_volume_behind_its_drive_letter`.
    let long_text = format!(r"C:\{}\file.txt", vec!["directory"; 24].join("\\"));
    let kept_long = Wtf16String::from(long_text.as_str());

    let mut timings = Vec::new();

    // The owned string a build is handed, on its own. Every build below
    // includes this clone, so it is what to subtract to read the rest.
    timings.push(time_loop("clone_path_string", ITERATIONS, || kept.clone()));

    // A request whose path is kept as given: an absolute path, at two lengths.
    // No directory is read.
    timings.push(time_loop("build_request_kept", ITERATIONS, || {
        open_request(&kept)
    }));
    timings.push(time_loop("build_request_kept_long", ITERATIONS, || {
        open_request(&kept_long)
    }));

    // A request whose path is grafted onto the current directory, which the
    // build reads.
    timings.push(time_loop("build_request_grafted", ITERATIONS, || {
        open_request(&relative)
    }));

    // Cloning a finished request, which is what a request-recycling scheme would
    // pay in place of a build. A grafted request holds two strings -- the path as
    // given and the path handed to Win32 -- where a kept one holds the one.
    let kept_request = open_request(&kept);
    let grafted_request = open_request(&relative);

    // The rows are named for what the path does, so the samples are checked to do
    // it: a sample that stopped being grafted, or one that began to be, would
    // leave a row measuring the other case under the wrong name.
    assert_eq!(
        kept_request.win32_path(),
        kept_request.path(),
        "the absolute sample is meant to be kept as given"
    );
    let kept_long_request = open_request(&kept_long);
    assert_eq!(
        kept_long_request.win32_path(),
        kept_long_request.path(),
        "the long absolute sample is meant to be kept as given"
    );
    assert_ne!(
        grafted_request.win32_path(),
        grafted_request.path(),
        "the relative sample is meant to be grafted onto the current directory"
    );

    timings.push(time_loop("clone_request_kept", ITERATIONS, || {
        kept_request
            .try_clone()
            .expect("a request with no handle or security copies")
    }));
    timings.push(time_loop("clone_request_grafted", ITERATIONS, || {
        grafted_request
            .try_clone()
            .expect("a request with no handle or security copies")
    }));

    // The call a request carries, against a real file: a built request
    // performed, and a request built and performed. The returned handle is
    // dropped inside the timed region, so each is an open-and-close cycle.
    timings.push(time_loop("perform_open", PERFORM_ITERATIONS, || {
        kept_request
            .perform()
            .expect("kernel32.dll opens for reading")
    }));
    timings.push(time_loop(
        "build_and_perform_open",
        PERFORM_ITERATIONS,
        || {
            open_request(&kept)
                .perform()
                .expect("kernel32.dll opens for reading")
        },
    ));

    // A handle a request carries, shared as a request now holds it. The file is
    // this process's own, so nothing here depends on the filesystem beyond
    // opening it once.
    let shared =
        SharedHandle::from(std::fs::File::open(&system_dll).expect("kernel32.dll is readable"));

    // A clone and its drop. `shared` outlives the loop, so a dropped clone only
    // counts one holder fewer and closes nothing: this is the whole cost of
    // giving a request a handle the caller already shares.
    timings.push(time_loop("share_handle", ITERATIONS, || shared.clone()));

    // The kept-path build again, now also carrying a template handle, to read
    // against `build_request_kept`. It is only built, never performed.
    timings.push(time_loop("build_request_with_handle", ITERATIONS, || {
        open_request(&kept).with_template(Some(shared.clone()))
    }));

    // What a caller holding only a borrow pays, outside the request crate, for a
    // handle it can share: a `DuplicateHandle`, a kernel transition. It is not a
    // cost of building a request and is labelled so.
    //
    // Duplicate and close are timed SEPARATELY, and that separation is the whole
    // point. `time_loop` black-boxes its closure's return value and drops it at
    // the end of the statement, so a loop that duplicates and returns an
    // `OwnedHandle` also calls `CloseHandle` inside the timed region. That is two
    // kernel transitions reported as one number, read as the cost of the
    // duplication alone.
    //
    // Retaining every duplicate in a pre-sized `Vec` keeps the close out of the
    // duplication loop, and timing the drop of that same `Vec` recovers the close
    // as its own figure rather than discarding it. The `push` is a pointer bump
    // into reserved capacity, which is not free but is nowhere near a syscall.
    let borrowed = shared.as_handle();

    // Warmed the same way `time_loop` warms, and for the same reason: the first
    // pass pays for lazily resolved syscall stubs and the allocator's first
    // touch of a fresh size class.
    for _ in 0..256 {
        let _ = std::hint::black_box(borrowed.try_clone_to_owned().expect("duplicating a handle"));
    }

    let mut duplicates: Vec<OwnedHandle> = Vec::with_capacity(HANDLE_ITERATIONS as usize);
    let start = Instant::now();
    for _ in 0..HANDLE_ITERATIONS {
        duplicates.push(
            borrowed
                .try_clone_to_owned()
                .expect("duplicating an owned handle"),
        );
    }
    let duplicate_elapsed = start.elapsed();
    timings.push(Timing {
        label: "duplicate_borrowed_handle",
        iterations: HANDLE_ITERATIONS,
        nanos_per_op: duplicate_elapsed.as_nanos() as f64 / f64::from(HANDLE_ITERATIONS),
    });

    let start = Instant::now();
    drop(duplicates);
    let close_elapsed = start.elapsed();
    timings.push(Timing {
        label: "close_duplicate",
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

        // Two conditions, two assertions, because only one of them is an OS
        // failure and an error code attached to the other would be fiction --
        // `GetLastError` is not meaningful for a call that returned a size.
        assert!(
            retried != 0,
            "GetSystemDirectoryW failed at the size it asked for ({written}): {}",
            std::io::Error::last_os_error()
        );
        assert!(
            retried < heap.len(),
            "GetSystemDirectoryW asked for {written} units, then wanted {retried} \
             at that size -- the system directory changed between the two calls"
        );
        return std::path::PathBuf::from(os_string(&heap[..retried]));
    }

    std::path::PathBuf::from(os_string(&buffer[..written]))
}

/// Losslessly, because a Windows path is UTF-16 and not necessarily Unicode.
fn os_string(units: &[u16]) -> std::ffi::OsString {
    std::os::windows::ffi::OsStringExt::from_wide(units)
}
