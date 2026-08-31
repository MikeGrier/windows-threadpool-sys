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
//! carry it (~165 ns, per `probe-doorbell-cost`)?
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
//! The conclusion it *does* support is about **operation mix**: for an
//! open-heavy workload, effort spent shaving the doorbell would be spent on the
//! small half of the cost.
//!
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
//! That means the measured cost is a *syscall* cost and cannot be tuned away by
//! an allocator. An inline-storage or recycling scheme would only recover the
//! allocation part, which `clone_prepared_units` bounds from below. Knowing
//! which half is which is the point of measuring both.
//!
//! [the namespace session]: ../../../design-sessions/DESIGN-SESSION-2026-08-27-pseudo-async-namespace-operations.md
//!
//! Each timing is reported per operation. Absolute values are host-specific;
//! the **ratios against the doorbell and the atomic** are the finding.

use std::time::Instant;

use wtf_string::Wtf16String;

use windows_namespace_request_sys::{CapturedHandle, OpenFile, prepare};
use windows_sys::Win32::Foundation::GENERIC_READ;
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, OPEN_EXISTING};

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

    let short = Wtf16String::from(r"C:\Windows\System32\kernel32.dll");
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
    let file =
        std::fs::File::open(r"C:\Windows\System32\kernel32.dll").expect("kernel32.dll is readable");
    let borrowed = std::os::windows::io::AsHandle::as_handle(&file);
    timings.push(time_loop("capture_handle", HANDLE_ITERATIONS, || {
        CapturedHandle::capture(borrowed).expect("duplicating an owned handle")
    }));

    Observation { timings }
}
