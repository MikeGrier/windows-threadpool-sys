// Copyright (c) 2026 Mike Grier
//! The harness-owned schedule wire format.
//!
//! `windows-file-watcher` does not serialize `Notification`, so the harness
//! defines its own serde-serializable description of a notification
//! ([`NotificationSpec`]) and converts it into a real `Notification` -- via the
//! `test-util` builders -- at drive time. That JSON is a tool I/O format, not a
//! data contract; its shape may change in any release.
//!
//! # The format is unvalidated (deliberately)
//!
//! A [`Schedule`] is plain data. Nothing here checks that a schedule is one
//! file-watcher could actually produce -- you can express a `Batch` after that
//! watch's `Completion { Cancelled }`, a `Desync { Overflow }` on a watch
//! established `Coarse`, or a `VolumeChanged` whose `previous` does not
//! continue from its own prior `current`. That permissiveness is intentional
//! (crate DESIGN-NOTES D-7): the same format must faithfully carry a *recorded*
//! schedule (whatever actually happened), so it cannot be pre-constrained to
//! only-legal. Staying inside file-watcher's contract is therefore the caller's
//! job -- the generator's (DESIGN-NOTES D-5) or yours when you author a schedule
//! by hand.
//!
//! For the sequencing rules that *are* mechanically checkable, hand a schedule's
//! notifications to [`windows_file_watcher::ContractChecker`] rather than
//! re-deriving them; the three examples above are exactly what it rejects. Be
//! careful which orderings you assume are illegal, though: a `Resumed` with no
//! prior `Suspended` looks wrong and is legal (a subscription joining an
//! already-faulted watcher never sees the `Suspended`), and two consecutive
//! questions for one watch are legal too, because the answer that separates them
//! travels the request queue, which this format does not represent.
//! The dependencies you must respect to stay legal follow.
//!
//! # Data dependencies
//!
//! - **`watch` correlates a subscription across steps.** Every notification
//!   carries a `watch` id; equal raw values become equal `WatchId`s, so `watch`
//!   is the field a handler uses to route or aggregate. A schedule for several
//!   subscriptions interleaves their notifications, and each watch's own
//!   sub-sequence is what carries ordering meaning -- the control-flow rules
//!   below are *per watch*.
//! - **A rename's two halves are independent, unjoined records.**
//!   [`ChangeKindSpec::RenamedOldName`] and [`ChangeKindSpec::RenamedNewName`]
//!   are raw, distinct kinds; `windows-file-watcher` never joins them or
//!   correlates them across a buffer (its own DESIGN-NOTES D-9). The kernel
//!   usually delivers them adjacent within one `Batch`, but nothing in the
//!   contract requires that: a legal schedule may carry a lone half (its mate
//!   arriving in a later batch, or never, e.g. lost to a `Desync`), both
//!   halves together, or other changes interleaved between them. A handler
//!   that assumes adjacency or pairing is assuming something
//!   `windows-file-watcher` does not promise.
//! - **`VolumeChanged`'s `previous`/`current` must have distinct serials.**
//!   `windows-file-watcher` only emits this notification when a reopen's
//!   volume identity actually differs from the one previously confirmed
//!   (file-watcher D-78), and identity compares by serial alone
//!   (`VolumeSpec::serial`, mirroring `VolumeIdentity`'s own `PartialEq`,
//!   file-watcher D-50). A `previous`/`current` pair with equal serials is not
//!   a legal schedule -- it describes a volume "changing" to itself, which the
//!   crate never reports. The generator enforces this by construction rather
//!   than by chance.
//! - **Carried detail must otherwise be self-consistent.** `Failed` and
//!   `RetryQuestion` carry a [`FaultDetailSpec`] the handler reads. Nothing
//!   across steps depends on its value, but it should be internally sensible
//!   (a real Win32 code).
//!
//! # Control-flow (sequencing) dependencies
//!
//! The driver delivers steps strictly in order, one at a time, on one thread, so
//! **schedule order is delivery order** -- all sequencing meaning lives in how
//! you order `steps`. For one watch, file-watcher's contract implies:
//!
//! - **Establishment precedes data, and (for a liveness watch, on immediate
//!   success) `Established` precedes `Completion`.** `windows-file-watcher`
//!   sends the initial `Established { mode }` from inside route
//!   establishment, and only afterward turns the result into the
//!   `Completion { Subscribed }` its caller reports -- so a liveness watch
//!   that establishes immediately has `Established` then
//!   `Completion { Subscribed }` as its first two notifications, never the
//!   reverse. A non-liveness watch that establishes immediately has only the
//!   `Completion`. Either way no `Batch` arrives before establishment
//!   (file-watcher D-30/D-13). "Nothing precedes establishment" holds for a
//!   watch that establishes against a *healthy* directory; one that coalesces
//!   onto an already-faulted watcher is the documented exception, and is
//!   covered in the liveness-bracket bullet below.
//!
//!   Two other first-registration outcomes exist, and this ordering rule does
//!   not apply to them: a **retryable first open** reports
//!   `Completion { Establishing }` instead -- and, for an interactive watch,
//!   `park_pending` (monitor.rs) sends that attempt's `RetryQuestion` *before*
//!   `Completion { Establishing }` is even reported, so the question can
//!   precede the completion it answers on behalf of, not only follow an
//!   already-`Establishing` watch's later `Completion { Subscribed }`. A
//!   **permanent initial failure** reports `Completion { Failed }` directly,
//!   with no `Established` and no prior `Completion` at all -- it is that
//!   watch's sole and terminal notification.
//! - **`Desync` is an in-stream barrier.** Everything before a `Desync` for a
//!   watch is accounted for; nothing after it is (file-watcher D-12). A schedule
//!   that models loss puts the `Desync` exactly at the drop point.
//! - **A fault and its resolution -- or its terminal outcome -- are one unit,
//!   with one documented exception.** Entering a fault sends `Suspended`
//!   (liveness only). A fault against an already-established watch (this
//!   bullet's scope; a not-yet-established watch's retryable open is the
//!   separate case documented above) ends one of two ways: **successful
//!   recovery** always sends `Desync { Reestablished }` (file-watcher D-12:
//!   unconditional, never gated on liveness), then -- for a liveness watch --
//!   `Resumed` followed by a fresh `Established`
//!   (file-watcher D-13/D-17/D-31); or **permanent
//!   failure to reopen** ends the watch instead with the terminal
//!   `Desync { Stopped }` (`record_stop`, watcher.rs) and nothing further for
//!   it. A watch that is neither liveness nor interactive still legally sees
//!   the bare successful-recovery resolution `Desync { Reestablished }` on a
//!   silent, autonomous recovery. No second, overlapping fault may appear
//!   inside the bracket.
//!
//!   `Resumed` and `Established` are **attempted** together --
//!   `resolve_fault_success` issues them back to back -- but each is a separate
//!   best-effort observation send (file-watcher D-57), so a saturated queue can
//!   take one and latch the other into a `Desync { QueueFull }`. "Always
//!   together" describes the attempt, not the delivery: a schedule carrying
//!   `Resumed` without `Established` is legal.
//!
//!   **The exception is a single `Batch`, and it is real rather than
//!   theoretical.** `WatcherInner::on_completion` re-arms *before* it decodes,
//!   so a read that completed successfully and then failed to re-arm calls
//!   `enter_fault` first -- emitting `Suspended`/`RetryQuestion` -- and only
//!   then publishes the batch that already completed. Those changes are real
//!   and were already in hand; dropping them to keep the bracket tidy would be
//!   the silent loss the whole design forbids. So exactly one `Batch`, carrying
//!   the completion that triggered the fault, may follow the bracket's opening
//!   notifications. A handler that treats any data inside a bracket as
//!   impossible is wrong about production.
//! - **A question, if any, is asked from inside that same bracket -- but is
//!   not always followed by a resolution.** A `RetryQuestion` or
//!   `VolumeChanged` is a question the client answers, and it only ever
//!   arises from inside a fault (file-watcher's `enter_fault`) -- never on
//!   its own. For an already-established watch's fault (the bracket
//!   documented immediately above), a question is always eventually followed
//!   by that bracket's resolution or its `Desync { Stopped }` terminal
//!   outcome. For a not-yet-established watch's retryable open, a further
//!   retry attempt can ask *another* `RetryQuestion` with no resolution
//!   between them (`monitor.rs`'s retry path), repeating until the watch
//!   either establishes (`Established`/`Completion { Subscribed }`) or
//!   permanently fails (`Completion { Failed }`, with no
//!   `Desync { Reestablished }` ever, since nothing was ever established). A
//!   watcher cannot fault twice concurrently, so a second question for the
//!   same watch does not appear before the first resolves, retries, or
//!   terminates (file-watcher D-28).
//! - **Three forms are terminal for a watch, not only `Cancelled`.** After
//!   `Completion { Cancelled }` (file-watcher D-30), `Completion { Failed }`
//!   (an establishment-or-continuation failure -- not only a permanent open
//!   failure on first registration, but also, for an already-routed watch, a
//!   route that cannot be migrated during identity-collision rekeying,
//!   monitor.rs's `rekey`), or `Desync { Stopped }` (a live watch that later
//!   became permanently unwatchable), nothing more arrives for that watch.
//!   Each is a per-watch terminator.
//!
//!   **One exception, and it is real output rather than a technicality:** a
//!   single `Desync { QueueFull }` may follow a terminator. A cancellation
//!   completion holds a slot reserved since registration (file-watcher D-45),
//!   so when saturation has latched a loss, the reserved send cannot flush that
//!   latch -- its own reservation is still counted -- and enqueues
//!   `Cancelled` ahead of it. The receiver drains the queue before synthesizing
//!   latches, so the owed report arrives after the terminator. Exactly one can
//!   be owed, since latching coalesces per watch. `ContractChecker` accepts that
//!   one and rejects a second.
//! - **`Established` recurs on re-establishment, and the tier may differ each
//!   time.** When liveness is on, `Established { mode }` appears once at first
//!   establishment and again after each re-establishment (file-watcher D-17),
//!   immediately after `Resumed` in the fault-recovery bracket above. The
//!   `mode` is **re-resolved on every reopen** (file-watcher D-61), so one
//!   watch may legally see `Established { Detailed }` and later
//!   `Established { Coarse }`, or the reverse -- detailed is attempted first
//!   every time, so a downgrade is not permanent. A handler that caches a
//!   watch's tier from the first `Established` is caching something the
//!   contract does not promise.
//! - **A liveness bracket does not always open with `Suspended`, and does not
//!   always close with `Resumed`.** Both halves of the pairing can be absent,
//!   so a schedule may legally contain either:
//!   - a **`Resumed` with no prior `Suspended`** for that watch. A subscription
//!     that coalesces onto a directory whose watcher is *already faulted* joins
//!     the route set after `enter_fault` has sent its `Suspended`s, so it never
//!     sees one; its first `Established` is suppressed as well (there is no
//!     settled tier to name), and it observes
//!     `Completion { Subscribed }` first, then `Desync { Reestablished }`,
//!     `Resumed`, and only then its first `Established`. So `Established` is
//!     **not** necessarily a watch's first notification.
//!   - a **`Suspended` closed by `Desync { Stopped }` with no `Resumed`**, per
//!     the terminal branch documented above.
//!
//!   A handler that tracks liveness by balancing `Suspended`/`Resumed` pairs is
//!   therefore wrong in both directions, which is exactly the kind of assumption
//!   this harness exists to test against.

use serde::{Deserialize, Serialize};
use windows_file_watcher::{
    Change, ChangeKind, DesyncCause, FailureCode, FaultDetail, FaultOperation, Notification,
    OpenFailure, Outcome, RelativeName, VolumeIdentity, WatchId, WatchMode,
};

/// An ordered, deterministic sequence of notifications to drive a handler with.
///
/// The schedule is the *sole* source of events during a run, which is what makes
/// a run reproducible, and its order *is* the delivery order (the driver never
/// reorders). Build one by hand for a targeted test, or generate one (later
/// milestones); either way it round-trips through JSON. See the [module
/// docs](self) for the data and control-flow dependencies a legal schedule must
/// respect.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// The notifications to deliver, in order.
    pub steps: Vec<NotificationSpec>,
}

impl Schedule {
    /// An empty schedule.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one step, returning `&mut self` for chaining.
    pub fn push(&mut self, spec: NotificationSpec) -> &mut Self {
        self.steps.push(spec);
        self
    }

    /// The number of steps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether the schedule has no steps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// A serde-serializable description of one notification to deliver.
///
/// `watch` is a raw subscription id you choose; the harness turns it into a
/// `WatchId` with `WatchId::from_raw`, so equal raw values correlate to the same
/// subscription across steps. Every arm maps one-to-one onto a `Notification`
/// variant. The ordering rules between these -- what may follow what, per watch
/// -- are the control-flow dependencies in the [module docs](self).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationSpec {
    /// -> `Notification::Batch`.
    Batch {
        /// The subscription these changes belong to.
        watch: u64,
        /// The changes, in order.
        changes: Vec<ChangeSpec>,
    },
    /// -> `Notification::Desync` (re-scan).
    Desync {
        /// The subscription affected.
        watch: u64,
        /// Why the gap arose.
        cause: DesyncCauseSpec,
    },
    /// -> `Notification::Completion`.
    Completion {
        /// The subscription the request concerned.
        watch: u64,
        /// What happened.
        outcome: OutcomeSpec,
    },
    /// -> `Notification::Suspended`.
    Suspended {
        /// The subscription affected.
        watch: u64,
    },
    /// -> `Notification::Resumed`.
    Resumed {
        /// The subscription affected.
        watch: u64,
    },
    /// -> `Notification::Established`.
    Established {
        /// The subscription affected.
        watch: u64,
        /// The tier now watching.
        mode: WatchModeSpec,
    },
    /// -> `Notification::RetryQuestion`.
    RetryQuestion {
        /// The subscription being asked.
        watch: u64,
        /// Which operation faulted.
        operation: FaultOperationSpec,
        /// The classification and code behind the failure.
        detail: FaultDetailSpec,
    },
    /// -> `Notification::VolumeChanged`.
    VolumeChanged {
        /// The subscription being asked.
        watch: u64,
        /// The volume previously confirmed.
        previous: VolumeSpec,
        /// The volume the reopen landed on.
        current: VolumeSpec,
    },
}

impl NotificationSpec {
    /// The subscription id this notification is tagged with.
    ///
    /// Every variant carries a `watch`; this reads it without matching each arm.
    #[must_use]
    pub fn watch(&self) -> u64 {
        match self {
            NotificationSpec::Batch { watch, .. }
            | NotificationSpec::Desync { watch, .. }
            | NotificationSpec::Completion { watch, .. }
            | NotificationSpec::Suspended { watch }
            | NotificationSpec::Resumed { watch }
            | NotificationSpec::Established { watch, .. }
            | NotificationSpec::RetryQuestion { watch, .. }
            | NotificationSpec::VolumeChanged { watch, .. } => *watch,
        }
    }

    /// Convert this description into a real `Notification` using file-watcher's
    /// `test-util` builders.
    #[must_use]
    pub fn to_notification(&self) -> Notification {
        match self {
            NotificationSpec::Batch { watch, changes } => Notification::Batch {
                watch: WatchId::from_raw(*watch),
                changes: changes.iter().map(ChangeSpec::to_change).collect(),
            },
            NotificationSpec::Desync { watch, cause } => Notification::Desync {
                watch: WatchId::from_raw(*watch),
                cause: cause.to_cause(),
            },
            NotificationSpec::Completion { watch, outcome } => Notification::Completion {
                watch: WatchId::from_raw(*watch),
                outcome: outcome.to_outcome(),
            },
            NotificationSpec::Suspended { watch } => Notification::Suspended {
                watch: WatchId::from_raw(*watch),
            },
            NotificationSpec::Resumed { watch } => Notification::Resumed {
                watch: WatchId::from_raw(*watch),
            },
            NotificationSpec::Established { watch, mode } => Notification::Established {
                watch: WatchId::from_raw(*watch),
                mode: mode.to_mode(),
            },
            NotificationSpec::RetryQuestion {
                watch,
                operation,
                detail,
            } => Notification::RetryQuestion {
                watch: WatchId::from_raw(*watch),
                operation: operation.to_operation(),
                detail: detail.to_detail(),
            },
            NotificationSpec::VolumeChanged {
                watch,
                previous,
                current,
            } => Notification::VolumeChanged {
                watch: WatchId::from_raw(*watch),
                previous: previous.to_identity(),
                current: current.to_identity(),
            },
        }
    }
}

/// -> `Change`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSpec {
    /// What kind of change.
    pub kind: ChangeKindSpec,
    /// The affected name, relative to the watched directory.
    pub name: NameSpec,
}

impl ChangeSpec {
    fn to_change(&self) -> Change {
        Change {
            kind: self.kind.to_kind(),
            name: self.name.to_relative_name(),
        }
    }
}

/// A change's name, losslessly. A plain UTF-8 string is enough for a
/// hand-authored schedule, but a *recorded* one must be able to carry
/// whatever the kernel actually reported (D-7) -- including a lone UTF-16
/// surrogate, which is a legal `RelativeName` (file-watcher D-83) but not
/// representable as a Rust `String` at all. `Units` exists for that case;
/// `Text` is the ergonomic common case, and both serialize the way you would
/// expect (a plain JSON string, or an array of `u16`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NameSpec {
    /// A well-formed UTF-8 name.
    Text(String),
    /// Raw UTF-16 units, kernel-shape -- the only form that can carry a lone
    /// surrogate.
    Units(Vec<u16>),
}

impl NameSpec {
    fn to_relative_name(&self) -> RelativeName {
        match self {
            NameSpec::Text(name) => RelativeName::for_test(name),
            NameSpec::Units(units) => RelativeName::for_test_units(units),
        }
    }
}

impl From<&str> for NameSpec {
    fn from(name: &str) -> Self {
        NameSpec::Text(name.to_string())
    }
}

impl From<String> for NameSpec {
    fn from(name: String) -> Self {
        NameSpec::Text(name)
    }
}

impl From<Vec<u16>> for NameSpec {
    fn from(units: Vec<u16>) -> Self {
        NameSpec::Units(units)
    }
}

/// -> `ChangeKind`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKindSpec {
    /// Added.
    Added,
    /// Removed.
    Removed,
    /// Modified.
    Modified,
    /// The old name of a rename.
    RenamedOldName,
    /// The new name of a rename.
    RenamedNewName,
    /// An action code file-watcher does not recognise, preserved verbatim.
    Unknown(u32),
}

impl ChangeKindSpec {
    fn to_kind(&self) -> ChangeKind {
        match self {
            ChangeKindSpec::Added => ChangeKind::Added,
            ChangeKindSpec::Removed => ChangeKind::Removed,
            ChangeKindSpec::Modified => ChangeKind::Modified,
            ChangeKindSpec::RenamedOldName => ChangeKind::RenamedOldName,
            ChangeKindSpec::RenamedNewName => ChangeKind::RenamedNewName,
            ChangeKindSpec::Unknown(code) => ChangeKind::Unknown(*code),
        }
    }
}

/// -> `DesyncCause`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesyncCauseSpec {
    /// Kernel buffer overflow or an unparseable completion.
    Overflow,
    /// The client's bounded queue was full.
    QueueFull,
    /// Coarse-mode watching reports only that something changed.
    Coarse,
    /// The watch was re-established after an outage.
    Reestablished,
    /// The watch stopped permanently.
    Stopped,
}

impl DesyncCauseSpec {
    /// The real [`DesyncCause`] this spec stands for.
    #[must_use]
    pub fn to_cause(&self) -> DesyncCause {
        match self {
            DesyncCauseSpec::Overflow => DesyncCause::Overflow,
            DesyncCauseSpec::QueueFull => DesyncCause::QueueFull,
            DesyncCauseSpec::Coarse => DesyncCause::Coarse,
            DesyncCauseSpec::Reestablished => DesyncCause::Reestablished,
            DesyncCauseSpec::Stopped => DesyncCause::Stopped,
        }
    }

    /// Whether a watch established in `mode` can ever report this cause.
    ///
    /// Delegates to [`DesyncCause::is_reachable_in`] rather than re-deriving the
    /// rule. That delegation is the point: this harness exists to hold itself to
    /// `windows-file-watcher`'s contract, and a second, hand-written copy of a
    /// contract rule is not a check of the contract but a check of the copy.
    /// Both directions of getting it wrong have already happened here.
    #[must_use]
    pub fn is_reachable_in(&self, mode: &WatchModeSpec) -> bool {
        self.to_cause().is_reachable_in(mode.to_mode())
    }
}

/// -> `Outcome`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutcomeSpec {
    /// Registered and watching.
    Subscribed,
    /// Registered but not yet openable.
    Establishing,
    /// Permanently failed.
    Failed {
        /// The classification and code behind the failure.
        detail: FaultDetailSpec,
    },
    /// The subscription ended.
    Cancelled,
}

impl OutcomeSpec {
    fn to_outcome(&self) -> Outcome {
        match self {
            OutcomeSpec::Subscribed => Outcome::Subscribed,
            OutcomeSpec::Establishing => Outcome::Establishing,
            OutcomeSpec::Failed { detail } => Outcome::Failed {
                detail: detail.to_detail(),
            },
            OutcomeSpec::Cancelled => Outcome::Cancelled,
        }
    }
}

/// -> `FaultDetail`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultDetailSpec {
    /// How the retry policy should treat this failure.
    pub failure: OpenFailureSpec,
    /// The raw code behind it.
    pub code: FailureCodeSpec,
}

impl FaultDetailSpec {
    fn to_detail(&self) -> FaultDetail {
        FaultDetail {
            failure: self.failure.to_failure(),
            code: self.code.to_code(),
        }
    }
}

/// -> `OpenFailure`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenFailureSpec {
    /// Nothing exists at the path (retryable).
    NotFound,
    /// Something exists but is not a directory (permanent).
    NotADirectory,
    /// The volume cannot support the detailed API.
    Unsupported,
    /// Anything else, retryable with backoff.
    Retryable,
    /// The path contains an interior NUL (permanent).
    InvalidPath,
    /// A retry timer could not be created (permanent for this attempt).
    RetryUnavailable,
}

impl OpenFailureSpec {
    fn to_failure(&self) -> OpenFailure {
        match self {
            OpenFailureSpec::NotFound => OpenFailure::NotFound,
            OpenFailureSpec::NotADirectory => OpenFailure::NotADirectory,
            OpenFailureSpec::Unsupported => OpenFailure::Unsupported,
            OpenFailureSpec::Retryable => OpenFailure::Retryable,
            OpenFailureSpec::InvalidPath => OpenFailure::InvalidPath,
            OpenFailureSpec::RetryUnavailable => OpenFailure::RetryUnavailable,
        }
    }
}

/// -> `FailureCode`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureCodeSpec {
    /// A `WIN32_ERROR`.
    Win32(u32),
    /// An `HRESULT`.
    HResult(i32),
}

impl FailureCodeSpec {
    fn to_code(&self) -> FailureCode {
        match self {
            FailureCodeSpec::Win32(code) => FailureCode::Win32(*code),
            FailureCodeSpec::HResult(code) => FailureCode::HResult(*code),
        }
    }
}

/// -> `WatchMode`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatchModeSpec {
    /// `ReadDirectoryChangesW`.
    Detailed,
    /// `FindFirstChangeNotification` fallback.
    Coarse,
}

impl WatchModeSpec {
    /// The real [`WatchMode`] this spec stands for.
    #[must_use]
    pub fn to_mode(&self) -> WatchMode {
        match self {
            WatchModeSpec::Detailed => WatchMode::Detailed,
            WatchModeSpec::Coarse => WatchMode::Coarse,
        }
    }
}

/// -> `FaultOperation`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FaultOperationSpec {
    /// The open faulted.
    Open,
    /// A read/rearm faulted.
    Arm,
}

impl FaultOperationSpec {
    fn to_operation(&self) -> FaultOperation {
        match self {
            FaultOperationSpec::Open => FaultOperation::Open,
            FaultOperationSpec::Arm => FaultOperation::Arm,
        }
    }
}

/// -> `VolumeIdentity` (via its `test-util` `for_test` builder).
///
/// Volume identity compares by serial alone; the descriptive fields are for
/// display.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSpec {
    /// The volume serial (the only field that affects identity equality).
    pub serial: u32,
    /// The filesystem name, e.g. `"NTFS"`.
    pub filesystem: String,
    /// The volume label.
    pub label: String,
}

impl VolumeSpec {
    fn to_identity(&self) -> VolumeIdentity {
        VolumeIdentity::for_test(self.serial, &self.filesystem, &self.label)
    }
}
