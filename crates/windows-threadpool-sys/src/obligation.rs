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

    /// Record the obligation, then publish the arming that creates it.
    ///
    /// The object is made live -- armed, started, or submitted -- by `publish`.
    /// From there until something settles it, a `Drop` would be doing work the
    /// caller could have done at a moment of their choosing.
    ///
    /// # The order is the whole point of this method
    ///
    /// `publish` makes the object reachable by the pool, so a dispatch can
    /// enter its trampoline and call [`record_settled`](Self::record_settled)
    /// at any instant after it returns -- on another thread, while this one is
    /// still between instructions. Recording afterwards therefore overwrites a
    /// settle that already happened, and leaves a completed object claiming it
    /// owes a drain. Under the `fail-fast` feature that is a panic on a
    /// correctly-drained object, and a panic during an unwinding `Drop` aborts.
    ///
    /// Recording first cannot fail the same way: the flag is set before
    /// anything can settle it, so a dispatch that consumes the arming clears it
    /// afterwards and the final value is the true one. The residual error is in
    /// the harmless direction -- an arming that is published late can be
    /// settled by an unrelated drain in between, which under-reports rather
    /// than panicking on an object that owes nothing.
    ///
    /// This is a method taking a closure, rather than a `record_live` the
    /// callers sequence themselves, because the sequencing was wrong at three
    /// of the eight sites that perform it (`wait`'s re-arm, `timer`'s deferred
    /// re-arm, and `work`'s submit) while being right at the other five. One
    /// site cannot disagree with itself.
    pub(crate) fn record_live_before(&self, publish: impl FnOnce()) {
        self.record_live();
        publish();
    }

    /// Set the flag.
    ///
    /// Deliberately private: [`record_live_before`](Self::record_live_before)
    /// is the only way to reach it from outside this module, so no caller can
    /// publish an arming and then record it. Reachable from the test module
    /// below, which asserts the ordering this privacy enforces.
    fn record_live(&self) {
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

/// Fail fast, if the `fail-fast` feature is on and a drain was owed.
///
/// Called at the **very end** of a `Drop`, after the drain, the close, and the
/// context free. That placement is the whole constraint: a panic unwinds, so
/// anything left after it would be skipped, and skipping the close would leave
/// the pool able to dispatch into a context nothing frees. The panic reports
/// the violation; it must not cause one. See [The teardown fail-fast is a
/// default-off Cargo feature](../../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature).
///
/// `owed` is read *before* the drain by every caller, because a callback
/// dispatched during the drain settles the obligation -- asking afterwards
/// would find nothing owed on exactly the objects that owed something.
///
/// One site for the message rather than five, so the wording a consumer sees
/// cannot differ by which type they dropped.
///
/// # What this does not catch, stated rather than left to be discovered
///
/// The flag records an **outstanding arming**, not "this `Drop` blocked". A
/// dispatch settles it for a wait and a one-shot timer -- the pool is no longer
/// watching once the callback has entered -- but `Drop` still waits for that
/// callback to finish. So dropping while a callback is executing performs a
/// blocking drain and reports nothing, and a callback that blocks for a long
/// time is exactly the case a consumer would most want told about.
///
/// This is a consequence of what the flag answers, not an oversight in it:
/// asking instead "is a callback running right now" is a question only the pool
/// can answer, and only by being asked at the moment `Drop` has already
/// committed to waiting. The feature therefore catches the obligation a caller
/// *left* -- an object still armed at `Drop` -- and not every teardown that
/// happens to block. `CleanupGroup` is outside it altogether: its teardown
/// drains, deliberately, but it carries no obligation flag and so never
/// reports.
pub(crate) fn fail_fast_if_owed(owed: bool, type_name: &str, close: &str) {
    #[cfg(feature = "fail-fast")]
    if owed {
        panic!(
            "windows-threadpool-sys: {type_name} was dropped with a drain still owed. Call \
             {close}() at a point you choose, rather than leaving the blocking drain to `Drop`. \
             This panic is the `fail-fast` feature; with it off the drop reports and continues."
        );
    }
    #[cfg(not(feature = "fail-fast"))]
    {
        let _ = (owed, type_name, close);
    }
}

/// Fail fast, if the `fail-fast` feature is on and a cancellation went
/// untracked.
///
/// `tracked` is what the registration reported -- `owe_repair` with `self-heal`
/// on, `owe_repair_claimed` with it off, which is why this names them rather
/// than linking: the feature-off `Registration` deliberately has no
/// `owe_repair`, so a link to one would be unresolvable in exactly the
/// configuration the `no self-heal` CI job documents. A
/// `false` means three allocations failed for one pool -- at the object's
/// creation, at any later registration by something else on it, and at the
/// retry the cancellation itself makes -- so the pool was cancelled with
/// nothing that will ever repair it. That is the single case
/// `try_cancel_pending`'s safety claim does not cover, and this is where a
/// build that asked to be told about it finds out.
///
/// **Called at a point where unwinding skips nothing that must happen**, which
/// is the same constraint [`fail_fast_if_owed`] carries and for the same
/// reason. `CleanupGroup::close_members_cancelling` marks its members between
/// the native release and the loop that frees their contexts; a panic raised
/// inside that loop would unwind past the frees and leak every context. So
/// callers collect the answer during the cancellation and call this afterwards,
/// never in the middle of one.
///
/// One site for the message rather than one per caller, so the wording cannot
/// drift between the per-object path and the group path.
pub(crate) fn fail_fast_if_untracked(tracked: bool, type_name: &str) {
    if tracked {
        return;
    }
    crate::trace_record!("heal", "cancel-untracked-reported", 0);
    #[cfg(feature = "fail-fast")]
    panic!(
        "windows-threadpool-sys: {type_name} cancelled pending callbacks on a pool this crate \
         could not register for repair, so nothing will repair it. Every attempt to allocate the \
         repair work item failed, including one made by this call. This panic is the `fail-fast` \
         feature; with it off the cancellation records `cancel-untracked` and continues."
    );
    #[cfg(not(feature = "fail-fast"))]
    {
        let _ = type_name;
    }
}

/// Fail fast, if the `fail-fast` feature is on, when a pool will not take a
/// repair.
///
/// Reached only from the self-heal timer, and only when a repair has sat with a
/// pool unstarted past the overdue threshold more than once with a reattempt in
/// between. At that point the crate has submitted the one action measured to
/// recover the stall, twice, and the pool has dispatched neither -- so it has no
/// remaining idea whether that pool can be recovered, which is the condition
/// this feature exists to stop on rather than continue past.
///
/// **This aborts rather than unwinds, which the other two fail-fast sites do
/// not.** It runs on the healer's timer callback, and a panic escaping an
/// `extern "system"` trampoline is turned into an abort by Rust -- see
/// `tests/callback_panic_aborts.rs`. The message still reaches stderr, because
/// the panic hook runs before unwinding begins. That is the honest outcome
/// here: the condition being reported is that a thread pool in this process has
/// stopped dispatching and would not take the repair, so there is no thread to
/// unwind to that is known to still work.
///
/// With the feature off the caller records the episode and re-submits, which it
/// will go on doing for as long as the pool stays stuck.
///
/// # Availability
///
/// Gated on `self-heal`, because the self-heal timer is its only caller: with
/// that feature off there is no repair to go unstarted and nothing to report.
#[cfg(feature = "self-heal")]
pub(crate) fn fail_fast_if_unrepairable(key: usize, overdue: u32) {
    #[cfg(feature = "fail-fast")]
    panic!(
        "windows-threadpool-sys: pool {key:#x} has not dispatched a repair after {overdue} \
         overdue attempts, so this crate cannot establish that it is still delivering callbacks. \
         A cancellation may have left it wedged and the repair this crate submits has not been \
         taken. This panic is the `fail-fast` feature; with it off the self-heal records \
         `repair-overdue` and keeps re-submitting."
    );
    #[cfg(not(feature = "fail-fast"))]
    {
        let _ = (key, overdue);
    }
}

/// The event every type emits when it finds an obligation owed at `Drop`.
///
/// One constant rather than the string at five call sites, so the tag a consumer
/// filters on cannot drift between them.
pub(crate) const DROP_OBLIGATION_OWED: &str = "drop-obligation-owed";

#[cfg(test)]
mod tests;
