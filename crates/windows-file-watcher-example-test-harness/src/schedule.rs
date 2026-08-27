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
//! file-watcher could actually produce -- you can express a `Resumed` with no
//! prior `Suspended`, two outstanding questions for one watch, or a `Batch`
//! after a `Cancelled`. That permissiveness is intentional (crate DESIGN-NOTES
//! D-7): the same format must faithfully carry a *recorded* schedule (whatever
//! actually happened), so it cannot be pre-constrained to only-legal; and the
//! legality rules are stateful *sequencing* constraints a per-value type cannot
//! enforce anyway. Staying inside file-watcher's contract is therefore the
//! caller's job -- the generator's (DESIGN-NOTES D-5) or yours when you author a
//! schedule by hand. The dependencies you must respect to stay legal follow.
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
//! - **Establishment precedes data, and (for a liveness watch) `Established`
//!   precedes `Completion`.** `windows-file-watcher` sends the initial
//!   `Established { mode }` from inside route establishment, and only
//!   afterward turns the result into the `Completion { Subscribed }` its
//!   caller reports -- so a liveness watch's first two notifications are
//!   `Established` then `Completion { Subscribed }`, never the reverse. A
//!   non-liveness watch has only the `Completion`. Either way, nothing
//!   precedes establishment, and no `Batch` arrives before it (file-watcher
//!   D-30/D-13).
//! - **`Desync` is an in-stream barrier.** Everything before a `Desync` for a
//!   watch is accounted for; nothing after it is (file-watcher D-12). A schedule
//!   that models loss puts the `Desync` exactly at the drop point.
//! - **A fault and its resolution are one unit; nothing else interleaves
//!   inside it.** Entering a fault sends `Suspended` (liveness only);
//!   resolving it always sends `Desync { Reestablished }` (file-watcher D-12:
//!   unconditional, never gated on liveness), then -- for a liveness watch,
//!   and always together, never one without the other -- `Resumed` followed by
//!   a fresh `Established` (file-watcher D-13/D-17/D-31). A watch that is
//!   neither liveness nor interactive still legally sees the bare resolution
//!   `Desync { Reestablished }` on a silent, autonomous recovery. No `Batch`,
//!   and no second, overlapping fault, may appear between a `Suspended` (or,
//!   for a non-liveness watch, the point a question is asked) and that
//!   resolution -- the watch is not armed while faulted, so it cannot be
//!   delivering data.
//! - **A question, if any, is asked from inside that same bracket.** A
//!   `RetryQuestion` or `VolumeChanged` is a question the client answers, and
//!   it only ever arises from inside a fault (file-watcher's `enter_fault`) --
//!   never on its own, and never without the resolution above eventually
//!   following it. A watcher cannot fault twice concurrently, so a second
//!   question for the same watch does not appear before the first resolves
//!   (file-watcher D-28).
//! - **`Cancelled` is terminal.** After `Completion { Cancelled }` for a watch,
//!   nothing more arrives for that watch (file-watcher D-30) -- it is a per-watch
//!   terminator.
//! - **`Established` recurs on re-establishment.** When liveness is on,
//!   `Established { mode }` appears once at first establishment and again after
//!   each re-establishment (file-watcher D-17), immediately after `Resumed` in
//!   the fault-recovery bracket above.

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
    pub name: String,
}

impl ChangeSpec {
    fn to_change(&self) -> Change {
        Change {
            kind: self.kind.to_kind(),
            name: RelativeName::for_test(&self.name),
        }
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
    fn to_cause(&self) -> DesyncCause {
        match self {
            DesyncCauseSpec::Overflow => DesyncCause::Overflow,
            DesyncCauseSpec::QueueFull => DesyncCause::QueueFull,
            DesyncCauseSpec::Coarse => DesyncCause::Coarse,
            DesyncCauseSpec::Reestablished => DesyncCause::Reestablished,
            DesyncCauseSpec::Stopped => DesyncCause::Stopped,
        }
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
    fn to_mode(&self) -> WatchMode {
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
