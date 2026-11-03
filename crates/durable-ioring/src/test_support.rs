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

use crate::capacity::Capacity;
use crate::clock::StampClock;
use crate::error_code::ErrorCode;
use crate::types::IoFault;

mod tests;

/// The small declaration the crate's tests build a core with (DI-D-107): enough that no test not
/// about capacity reaches it, and few enough that one about capacity can.
pub(crate) const CAPACITY: Capacity = Capacity {
    operations: 64,
    lineages: 16,
    imports: 8,
    targets: 8,
    domains: 16,
};

/// A clock that counts its readings: each one is a number higher than the last, so a core's
/// stamps are plain numbers and a test can read them off in order. Clones share one count.
#[derive(Clone, Default)]
pub(crate) struct Ticks(Arc<AtomicU64>);

impl StampClock for Ticks {
    type Instant = u64;

    fn now(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
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
        refusal: None,
        error: crate::report_io_error(error),
    }
}

/// A lineage as the enumeration lent it, owned so a test can keep it past the visit.
#[derive(Clone, Debug)]
pub(crate) struct Listed<V: crate::contract::Identities> {
    pub(crate) lineage: V::Lineage,
    pub(crate) description: Option<String>,
    pub(crate) is_default: bool,
    pub(crate) durable_through: Option<V::EpochId>,
}

impl<V: crate::contract::Identities> From<crate::types::LineageInfo<'_, V>> for Listed<V> {
    fn from(info: crate::types::LineageInfo<'_, V>) -> Self {
        Self {
            lineage: info.lineage,
            description: info.description.map(str::to_owned),
            is_default: info.is_default,
            durable_through: info.durable_through,
        }
    }
}
