// Copyright (c) 2026 Mike Grier
//! The portable doubles the core's tests use (DI-8.1): a stamp clock in plain numbers, and a fault
//! with no ring behind it. A test of the core, the oracle or the in-memory backend reads no
//! interrupt time and builds no Windows fault, so it builds wherever the portable crate does.
//!
//! The Windows driver's own tests keep their `MockClock` (`dioring/tests.rs`), which implements
//! `win_time_sys::Clock`: that is a fact about the driver's time base, not about the core.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::clock::StampClock;
use crate::error_code::ErrorCode;
use crate::types::IoFault;

mod tests;

/// A clock that counts its readings: each one is a number higher than the last, so a core's
/// stamps are plain numbers and a test can read them off in order. Clones share one count.
#[derive(Clone, Default)]
pub(crate) struct Ticks(Arc<AtomicU64>);

impl StampClock for Ticks {
    type Instant = u64;

    fn now(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }

    /// A tick is a nanosecond.
    fn elapsed(&self, earlier: u64, later: u64) -> Duration {
        Duration::from_nanos(later.saturating_sub(earlier))
    }
}

/// A clock a test moves by hand. Reading it does not move it, and it only moves forwards, which
/// is the promise a [`StampClock`] makes. Clones share one reading, so a test keeps a clone to
/// move the clock it gave away.
#[derive(Clone, Debug, Default)]
pub(crate) struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// A clock reading `ticks`.
    pub(crate) fn at(ticks: u64) -> Self {
        Self(Arc::new(AtomicU64::new(ticks)))
    }

    /// Move the clock forwards by `ticks`.
    pub(crate) fn advance(&self, ticks: u64) {
        self.0.fetch_add(ticks, Ordering::SeqCst);
    }
}

impl StampClock for ManualClock {
    type Instant = u64;

    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// A tick is a nanosecond.
    fn elapsed(&self, earlier: u64, later: u64) -> Duration {
        Duration::from_nanos(later.saturating_sub(earlier))
    }
}

/// The source a fault from [`fault`] names: no real layer, so no test can come to depend on one
/// layer's name.
pub(crate) const SOURCE: &str = "test";

/// A fault for `error`, as a backend would relay it: `SOURCE` for the layer, and the code an
/// error carrying an OS code names, taken as the number it is: this layer is no real one and
/// numbers its codes as it likes. An error with no OS code carries none.
pub(crate) fn fault(error: io::Error) -> IoFault {
    IoFault {
        source: SOURCE,
        code: error.raw_os_error().map(ErrorCode::from_hresult),
        error: Arc::new(error),
    }
}
