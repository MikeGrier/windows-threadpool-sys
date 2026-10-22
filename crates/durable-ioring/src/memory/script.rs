// Copyright (c) 2026 Mike Grier
//! The in-memory backend's fault script (DI-11.1, DI-D-50 P13, DI-D-74): a plan, written ahead of
//! a run, of what the backend and its targets do wrong, and when.
//!
//! A failing flush cannot be produced on a healthy machine, and a consumer's failure paths have
//! nothing to run against until one can be. A [`FaultScript`] is a list of steps, each a [`When`] and
//! a [`Fault`]: the nth write to a target fails, a commit fails, a completion is held until the test
//! releases it, a push is refused as though a queue were full. Nothing is random and nothing
//! depends on timing, so a run with a script is the same run every time.
//!
//! **Where a step applies.** A fault belongs to one point in the backend, which is how its [`When`]
//! counts: a refusal counts the pushes the backend would otherwise have admitted; a failure, a short
//! count, a hold or a late completion counts the submissions the doorbell executes; a refused doorbell
//! counts doorbells; a failed pop counts pops. Every step that matches a point counts it, whether or
//! not an earlier step fired for it, so a step's count does not depend on the steps before it; when
//! more than one fires for the same submission, the earliest in the script is the one applied.
//!
//! **The script does not make a refusal the backend would have made.** A push the backend refuses
//! for a real reason -- an unknown target, an access or geometry the target does not admit, a full
//! queue -- is refused for that reason, before the script is consulted, and is not counted.

use crate::backend::{RefusalCause, SubmissionOp, SubmissionToken};
use crate::types::{IoFault, IoTargetKey};

#[cfg(test)]
mod tests;

/// Which kind of operation a step applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptOp {
    /// A write.
    Write,
    /// A read.
    Read,
    /// A commit.
    Commit,
}

impl ScriptOp {
    /// The kind of `op`.
    pub(super) fn of<B>(op: &SubmissionOp<B>) -> Self {
        match op {
            SubmissionOp::Write { .. } => Self::Write,
            SubmissionOp::Read { .. } => Self::Read,
            SubmissionOp::Commit => Self::Commit,
        }
    }
}

/// Which of the occurrences a step matches it fires on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Occurrence {
    /// Every one.
    Every,
    /// Only the nth, counting from one.
    Nth(u32),
    /// The nth and every one after it.
    From(u32),
}

/// When a step fires: which operations it matches, and which of those it fires on.
///
/// Built from [`any`](Self::any) and narrowed: by the kind of operation, by the target, and by
/// occurrence, counting from one. With no occurrence given, a step fires on every match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct When {
    op: Option<ScriptOp>,
    target: Option<IoTargetKey>,
    occurrence: Occurrence,
}

impl When {
    /// Every operation on every target, every time.
    #[must_use]
    pub const fn any() -> Self {
        Self {
            op: None,
            target: None,
            occurrence: Occurrence::Every,
        }
    }

    /// Only operations of this kind.
    #[must_use]
    pub const fn op(mut self, op: ScriptOp) -> Self {
        self.op = Some(op);
        self
    }

    /// Only operations on this target.
    #[must_use]
    pub const fn target(mut self, target: IoTargetKey) -> Self {
        self.target = Some(target);
        self
    }

    /// Only the `n`th match, counting from one, and no other.
    ///
    /// # Panics
    ///
    /// If `n` is zero: occurrences are counted from one.
    #[must_use]
    pub const fn nth(mut self, n: u32) -> Self {
        assert!(n > 0, "occurrences are counted from one");
        self.occurrence = Occurrence::Nth(n);
        self
    }

    /// The `n`th match, counting from one, and every match after it: a device that fails from
    /// then on.
    ///
    /// # Panics
    ///
    /// If `n` is zero: occurrences are counted from one.
    #[must_use]
    pub const fn from(mut self, n: u32) -> Self {
        assert!(n > 0, "occurrences are counted from one");
        self.occurrence = Occurrence::From(n);
        self
    }

    /// Whether an operation of this kind on this target is one the step matches. `None` for
    /// either is a point with no operation, which a step naming one does not match.
    fn matches(&self, op: Option<ScriptOp>, target: Option<IoTargetKey>) -> bool {
        self.op.is_none_or(|wanted| op == Some(wanted))
            && self.target.is_none_or(|wanted| target == Some(wanted))
    }

    /// Whether the step fires on its `seen`th match, counting from one.
    fn fires(&self, seen: u32) -> bool {
        match self.occurrence {
            Occurrence::Every => true,
            Occurrence::Nth(n) => seen == n,
            Occurrence::From(n) => seen >= n,
        }
    }

    /// Whether the step names an operation or a target.
    fn is_narrowed(&self) -> bool {
        self.op.is_some() || self.target.is_some()
    }
}

/// What goes wrong when a step fires.
///
/// What each does to the operation, the target and the completion is stated on the variant. A
/// fault a step cannot apply to the kind of operation it matched is stated there too.
#[derive(Clone, Debug)]
pub enum Fault {
    /// The push is refused for this cause, and the submission comes back unchanged. Refusing as
    /// [`SubmitNeeded`](RefusalCause::SubmitNeeded), [`DrainNeeded`](RefusalCause::DrainNeeded) or
    /// [`SubmitAndDrainNeeded`](RefusalCause::SubmitAndDrainNeeded) is a full queue that is not one.
    Refuse(RefusalCause),
    /// The operation does not reach its target and completes with this fault. A write leaves the
    /// target's bytes as they were and a commit leaves its durable bytes as they were, which is a
    /// flush that failed; the submission's owned buffer comes back with the completion.
    Fail(IoFault),
    /// The operation reaches its target, and completes with this fault anyway: a write whose bytes
    /// are in the target but are reported failed, a commit that made the bytes durable and is
    /// reported failed. A read is a read that failed, as for [`Fail`](Self::Fail).
    FailAfterApplying(IoFault),
    /// The operation transfers at most this many bytes and completes successfully with the count it
    /// transferred. A write that completes short breaks the obligation that a write completes in
    /// full or with a fault (DI-D-50 P2), which is what makes it a source for testing what handles
    /// a short count, and what the conformance oracle refuses; a short read is an ordinary one.
    /// A commit has no count: a step naming commits cannot be given this, and a step naming no
    /// operation does not apply it to one.
    Short(u32),
    /// The operation executes as it would, and its completion is withheld until the test releases
    /// it with [`MemoryBackend::release`](super::MemoryBackend::release): a completion that is
    /// late for as long as the test wants, and out of order against the ones released before it.
    Hold,
    /// As [`Hold`](Self::Hold), released by the backend itself at the end of the doorbell this many
    /// doorbells after the one that executed it. Zero is an ordinary completion.
    Late(u32),
    /// The doorbell fails with this cause and issues nothing: what was queued stays queued. A step
    /// naming this names no operation or target.
    RefuseDoorbell(RefusalCause),
    /// The pop fails with this fault, and no completion is taken: what was ready stays ready. A step
    /// naming this names no operation or target.
    FailPop(IoFault),
}

/// The place in the backend a fault applies at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    /// A push the backend would otherwise admit.
    Push,
    /// A submission the doorbell executes.
    Execute,
    /// A doorbell.
    Doorbell,
    /// A pop.
    Pop,
}

impl Fault {
    /// Where it applies.
    pub(super) fn point(&self) -> Point {
        match self {
            Self::Refuse(_) => Point::Push,
            Self::Fail(_)
            | Self::FailAfterApplying(_)
            | Self::Short(_)
            | Self::Hold
            | Self::Late(_) => Point::Execute,
            Self::RefuseDoorbell(_) => Point::Doorbell,
            Self::FailPop(_) => Point::Pop,
        }
    }
}

/// One step of a script.
#[derive(Clone, Debug)]
struct Step {
    when: When,
    fault: Fault,
}

/// A script of faults for the in-memory backend, given to it with
/// [`MemoryBackend::with_script`](super::MemoryBackend::with_script).
///
/// A script is plain data and carries no count: the counts belong to the backend it is given to, so
/// one script can drive many backends, each from the start.
///
/// # Example
///
/// ```
/// use durable_ioring::memory::{scripted_fault, Fault, FaultScript, ScriptOp, When};
///
/// // The second commit on any target fails, and every write after the third is held.
/// let script = FaultScript::new()
///     .on(
///         When::any().op(ScriptOp::Commit).nth(2),
///         Fault::Fail(scripted_fault("the device was removed")),
///     )
///     .on(When::any().op(ScriptOp::Write).from(4), Fault::Hold);
/// assert_eq!(script.len(), 2);
/// ```
#[derive(Clone, Debug, Default)]
pub struct FaultScript {
    steps: Vec<Step>,
}

impl FaultScript {
    /// A script with no steps, which does nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a step: `fault`, when `when` says. Steps are consulted in the order they were added.
    ///
    /// # Panics
    ///
    /// For a step that cannot mean anything: [`Fault::Short`] on commits, which have no count, and
    /// [`Fault::RefuseDoorbell`] or [`Fault::FailPop`] with an operation or a target, neither of
    /// which a doorbell or a pop has. These are a script's author's mistakes, found where the
    /// script is written and not when a run silently ignores them.
    #[must_use]
    pub fn on(mut self, when: When, fault: Fault) -> Self {
        assert!(
            !(matches!(fault, Fault::Short(_)) && when.op == Some(ScriptOp::Commit)),
            "a commit has no count, so it cannot complete short"
        );
        assert!(
            !(matches!(fault.point(), Point::Doorbell | Point::Pop) && when.is_narrowed()),
            "a doorbell or a pop has no operation or target to name"
        );
        self.steps.push(Step { when, fault });
        self
    }

    /// How many steps it has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether it has no steps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// What a step fired for, recorded in the order the steps fired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fired {
    /// The step's position in the script, counting from zero.
    pub step: usize,
    /// The submission it fired for; `None` for a doorbell or a pop.
    pub token: Option<SubmissionToken>,
    /// The kind of that submission's operation.
    pub op: Option<ScriptOp>,
    /// That submission's target.
    pub target: Option<IoTargetKey>,
}

/// What a point was reached with.
#[derive(Clone, Copy, Debug)]
pub(super) struct Probe {
    pub(super) token: Option<SubmissionToken>,
    pub(super) op: Option<ScriptOp>,
    pub(super) target: Option<IoTargetKey>,
}

impl Probe {
    /// A doorbell or a pop, which has no submission.
    pub(super) fn bare() -> Self {
        Self {
            token: None,
            op: None,
            target: None,
        }
    }

    /// A submission.
    pub(super) fn of<B>(token: SubmissionToken, target: IoTargetKey, op: &SubmissionOp<B>) -> Self {
        Self {
            token: Some(token),
            op: Some(ScriptOp::of(op)),
            target: Some(target),
        }
    }
}

/// A script and the counts a backend has made against it.
#[derive(Debug, Default)]
pub(super) struct ScriptState {
    script: FaultScript,
    /// How many times each step has matched, by step.
    seen: Vec<u32>,
    fired: Vec<Fired>,
}

impl ScriptState {
    /// `script`, counted from the start.
    pub(super) fn new(script: FaultScript) -> Self {
        Self {
            seen: vec![0; script.steps.len()],
            script,
            fired: Vec::new(),
        }
    }

    /// What `point`, reached with `probe`, does: every step at that point that matches counts it,
    /// and the earliest that fires is the fault, if any.
    pub(super) fn consult(&mut self, point: Point, probe: Probe) -> Option<Fault> {
        let mut firing = None;
        for (index, step) in self.script.steps.iter().enumerate() {
            if step.fault.point() != point || !step.when.matches(probe.op, probe.target) {
                continue;
            }
            self.seen[index] = self.seen[index].saturating_add(1);
            if firing.is_none() && step.when.fires(self.seen[index]) {
                firing = Some(index);
            }
        }
        let index = firing?;
        self.fired.push(Fired {
            step: index,
            token: probe.token,
            op: probe.op,
            target: probe.target,
        });
        Some(self.script.steps[index].fault.clone())
    }

    /// What has fired, in order.
    pub(super) fn fired(&self) -> &[Fired] {
        &self.fired
    }
}
