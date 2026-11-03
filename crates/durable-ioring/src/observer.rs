// Copyright (c) 2026 Mike Grier
//! The failure observer (DI-D-113): what a consumer may be told of each failure as it happens, so
//! it can attribute a failure to its own operations, lineages and epochs in its own storage. The
//! ring keeps only what its own answers need; the detail is the observer's to keep, if it wants it.

use crate::contract::Identities;
use crate::types::{FailureCause, Marking, SuspectWrite};

/// Told of each failure an instance observes, and of what becomes of the writes it suspects, as
/// it happens (DI-D-113). Given when the instance is built, and only then: one observer, its
/// fan-out the consumer's, as for diagnostics (DI-D-96).
///
/// **When it is called.** For each failure, first [`failed`](Self::failed), then
/// [`suspected`](Self::suspected) once for each write it suspects, in push order -- all before its
/// `Failed` entry can be popped, so a consumer that pops it can find its own record. Then, for
/// each suspect write that is marked, [`marked`](Self::marked) once for each failure it goes to,
/// in the order the failures were observed, as the marking is made; a failure already resolved
/// is told nothing more.
///
/// **What it may do.** It is called synchronously, while the instance's lock is held: it must not
/// call back into the instance, and any lock it takes is a leaf beneath the instance's, as a
/// diagnostic sink's is (DI-D-90). What it is lent it copies if it keeps it. What it allocates is
/// its own (DI-D-97), and an allocation-counting test sees it under the core's call to it.
pub trait FailureObserver<V: Identities>: Send + Sync {
    /// Failure `failure` was observed, for `cause`, lent for the call, at `observed` -- the stamp
    /// its `Failed` entry carries.
    fn failed(&self, failure: V::FailureId, cause: &FailureCause<V>, observed: V::Instant);

    /// `failure` suspects `write`: its suspect set, one write at a time, in push order.
    fn suspected(&self, failure: V::FailureId, write: SuspectWrite<V>);

    /// A write `failure` suspects gained `marking`, lent for the call (DI-D-36).
    fn marked(&self, failure: V::FailureId, marking: &Marking<V>);
}
