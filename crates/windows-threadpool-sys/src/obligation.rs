// Copyright (c) Mike Grier
//! Whether a thread-pool object still owes the drain that `Drop` would do for it.
//!
//! Every teardown in this crate blocks to drain, per [the teardown-drains
//! decision](../../../DESIGN-NOTES.md#teardown-drains). Each type also offers a
//! synchronous close performing that same drain at a point the caller chooses,
//! because where `Drop` lands in the caller's control flow is often accidental.
//! This records which of the two is going to happen, so `Drop` can report having
//! been left the work.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether an undischarged drain obligation is outstanding.
///
/// The polarity is "owed", not "drained", and that is the whole reason this is a
/// type rather than a bare `bool` on each struct. An object that was created and
/// never armed owes nothing -- there is no callback for `Drop` to wait on -- so
/// the quiet state has to be the initial one. Recording "the caller called the
/// close" instead would make every never-armed object report.
///
/// # Ordering
///
/// Every access is `Relaxed`, which is sufficient here and would not be if this
/// guarded data. It guards none: no load of this flag decides whether to touch
/// memory, only whether to emit a trace record. And the read happens in `Drop`,
/// which takes `&mut self`, so every earlier `&self` call that set or cleared it
/// is already ordered before the read by exclusive access -- there is no race to
/// order, only a value to observe.
pub(crate) struct CloseObligation(AtomicBool);

impl CloseObligation {
    /// A fresh object owes nothing until something makes it live.
    pub(crate) const fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    /// The object was made live: armed, started, or submitted.
    ///
    /// From here until something settles it, a `Drop` would be doing work the
    /// caller could have done at a moment of their choosing.
    pub(crate) fn record_live(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// The obligation is discharged.
    ///
    /// Two different things reach this. The caller running the synchronous close
    /// is the one the report exists to encourage. A dispatch that *consumes* the
    /// arming is the other: a one-shot wait or timer is not armed again when its
    /// callback has run, so nothing is left to drain and there is nothing to
    /// report. A periodic timer is deliberately not in that second group -- the
    /// pool re-arms it, so a tick settles nothing.
    pub(crate) fn record_settled(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    /// Whether `Drop` is about to drain something the caller left to it.
    pub(crate) fn is_owed(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// The event every type emits when it finds an obligation owed at `Drop`.
///
/// One constant rather than the string at five call sites, so the tag a consumer
/// filters on cannot drift between them.
pub(crate) const DROP_OBLIGATION_OWED: &str = "drop-obligation-owed";

#[cfg(test)]
mod tests;
