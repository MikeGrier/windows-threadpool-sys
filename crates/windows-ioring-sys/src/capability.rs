// Copyright (c) 2026 Mike Grier
//! Capability query and version negotiation (M1.3, D-6).

use std::io;

use windows_sys::Win32::Storage::FileSystem::{
    IORING_CAPABILITIES, IORING_FEATURE_SET_COMPLETION_EVENT, IORING_FEATURE_UM_EMULATION,
    IORING_VERSION_1, IORING_VERSION_2, IORING_VERSION_3, QueryIoRingCapabilities,
};

use crate::error::check;

/// An `IORING_VERSION` value.
///
/// Wraps the raw value rather than a closed Rust enum, because the running
/// system can report a version this crate does not yet name: a spike found
/// `QueryIoRingCapabilities` reporting `MaxVersion = 400` on a machine current
/// as of this writing, while `windows-sys` 0.61.2 only names up to
/// [`RingVersion::V3`] (`IORING_VERSION_3 = 300`). Hardcoding the highest
/// named version as a hard ceiling would silently cap this crate the moment
/// a newer system shipped, rather than only until the next `windows-sys`
/// bump.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RingVersion(i32);

impl RingVersion {
    /// `IORING_VERSION_1`.
    pub const V1: RingVersion = RingVersion(IORING_VERSION_1);
    /// `IORING_VERSION_2`.
    pub const V2: RingVersion = RingVersion(IORING_VERSION_2);
    /// `IORING_VERSION_3`.
    pub const V3: RingVersion = RingVersion(IORING_VERSION_3);

    /// The highest version this crate can name. [`IoRing::new`](crate::IoRing::new)
    /// negotiates down to `min(HIGHEST_KNOWN, capabilities()?.max_version)`
    /// rather than assuming this is what the running system supports.
    pub const HIGHEST_KNOWN: RingVersion = Self::V3;

    /// Wrap a raw `IORING_VERSION` value, for example one reported by
    /// [`Capabilities::max_version`] that this crate does not name.
    #[must_use]
    pub fn from_raw(value: i32) -> Self {
        Self(value)
    }

    /// The raw `IORING_VERSION` value.
    #[must_use]
    pub fn raw(self) -> i32 {
        self.0
    }
}

/// What the running system's `IoRing` implementation supports, queried
/// without creating a ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The highest `IORING_VERSION` the system supports.
    pub max_version: RingVersion,
    /// The largest submission queue a ring can request.
    pub max_submission_queue_size: u32,
    /// The largest completion queue a ring can request.
    pub max_completion_queue_size: u32,
    /// Whether `SetIoRingCompletionEvent` is available. Both
    /// [`crate::IoRing::completion_event`] and the thread-pool delivery path
    /// built on it refuse to construct without this.
    pub supports_completion_event: bool,
    /// Whether the ring is emulated in user mode rather than backed by the
    /// kernel. A consumer reaching for this crate to maximize throughput
    /// needs to know when they are getting an emulation with no kernel
    /// benefit, so this is surfaced rather than hidden.
    pub is_emulated: bool,
}

/// Query the running system's `IoRing` capabilities.
///
/// Needs no ring: `QueryIoRingCapabilities` is a free-standing query, so a
/// consumer can decide whether to use this crate at all before creating
/// anything.
///
/// # Errors
///
/// Returns any error from `QueryIoRingCapabilities`.
pub fn capabilities() -> io::Result<Capabilities> {
    let mut raw = IORING_CAPABILITIES::default();
    // SAFETY: `raw` is a valid, appropriately sized out-pointer; the call
    // needs no ring.
    let hr = unsafe { QueryIoRingCapabilities(&raw mut raw) };
    check(hr)?;
    Ok(decode(&raw))
}

/// Turn a raw `IORING_CAPABILITIES` into this crate's shape.
///
/// Split out of [`capabilities`] so the flag decoding can be exercised with
/// values the running system does not report (M18.7). `QueryIoRingCapabilities`
/// reports whatever the host supports and nothing can vary it, so while this
/// lived inside the query it was unreachable from any test -- M18.3's mutation
/// run found all four of its bit operations surviving, including the one
/// deciding `supports_completion_event`, which gates every completion-event
/// path in the crate ([D-20](../DESIGN-NOTES.md#d-20)) and which
/// [#47](https://github.com/MikeGrier/windows-threadpool-sys/issues/47) lived
/// downstream of.
///
/// Pure, and takes the raw struct by reference: no syscall, no global state, so
/// a test can hand it any flag combination including ones no Windows build
/// produces.
fn decode(raw: &IORING_CAPABILITIES) -> Capabilities {
    Capabilities {
        max_version: RingVersion(raw.MaxVersion),
        max_submission_queue_size: raw.MaxSubmissionQueueSize,
        max_completion_queue_size: raw.MaxCompletionQueueSize,
        // Each flag is tested on its own bit: a mask that happens to carry
        // other feature bits must not change either answer.
        supports_completion_event: raw.FeatureFlags & IORING_FEATURE_SET_COMPLETION_EVENT != 0,
        is_emulated: raw.FeatureFlags & IORING_FEATURE_UM_EMULATION != 0,
    }
}

#[cfg(test)]
mod tests;
