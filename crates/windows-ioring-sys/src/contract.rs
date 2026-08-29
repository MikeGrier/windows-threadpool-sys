// Copyright (c) 2026 Mike Grier
//! [`RingContract`]: this crate's conservation rules, made executable.
//!
//! # Why an oracle rather than more assertions
//!
//! Some of what this crate promises is a property of a *sequence*, not of any
//! single value or call, and a per-value type cannot carry it. "Every SQE that
//! successfully queues produces exactly one completion" is only observable by
//! counting pushes against completions across a whole run; so is "no token was
//! dropped unclaimed", and so is "no registered buffer is still in use once
//! everything has finished".
//!
//! Those three are stated in `DESIGN-NOTES.md` and, before this module,
//! checked nowhere. That is not a hypothetical gap: two real defects in this
//! repository were conservation failures. `Appender::claim` returned early on
//! a failed write and permanently leaked the arena slot its token held, and a
//! strategy harness shared one deferred-commit slot between two lanes so half
//! its commits were never awaited. Both were found by review and by
//! measurement respectively, and both would have fallen out of a quiescence
//! check automatically.
//!
//! # Why it lives here
//!
//! The layer that owns an invariant owns the oracle for it. A copy written in
//! a test harness is a second implementation of the rule rather than a check
//! of it -- if the two disagree, the harness is what gets "fixed", and the
//! disagreement is precisely the bug worth finding. Being public means a
//! consumer can hold its own harness, its own test doubles, or a captured run
//! to the same definition this crate's tests use.
//!
//! # What it checks
//!
//! | Rule | Where it comes from |
//! |---|---|
//! | Every queued SQE produces exactly one completion | [`DESIGN-NOTES.md`'s category-2 audit](../DESIGN-NOTES.md#one-sqe-one-completion) |
//! | A push that failed synchronously produces none | the same section: it is un-counted, not merely uncompleted |
//! | No completion arrives for an operation never pushed | corollary of the above |
//! | Every token is claimed, or deliberately leaked | `Token`'s leak-on-drop contract ([D-13](../DESIGN-NOTES.md#d-13)) |
//! | Nothing is outstanding at quiescence | what `IoRing::run_down`'s termination depends on |
//!
//! # What it deliberately does **not** check
//!
//! Being explicit about this matters as much as the list above: over-
//! constraining is the same defect as under-specifying, and an oracle that
//! reports a violation which is not one trains its reader to ignore it.
//!
//! - **Completion order.** The ring makes no ordering promise between
//!   independent operations, so this counts them and never sequences them.
//!   The one ordering that *is* promised -- a covering flush against preceding
//!   operations ([D-24](../DESIGN-NOTES.md#d-24)) -- is not observable from
//!   the completion stream, because a barrier constrains when operations
//!   *execute*, not the order their completions are popped.
//! - **Whether an operation succeeded.** A failed operation still completes
//!   exactly once; conservation and success are different questions.
//! - **Anything about the device.** Whether a flush truly reached durable
//!   media is invisible here and everywhere else short of a power cut.
//! - **That a caller pushed anything sensible.** This checks conservation of
//!   what was pushed, not that pushing it was a good idea.

use std::collections::HashMap;
use std::fmt;

/// A conservation rule this crate promises, broken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A completion arrived carrying `user_data` that no observed push
    /// produced.
    ///
    /// Either the ring completed something twice, or the caller's own
    /// bookkeeping lost a push -- and distinguishing those is why the message
    /// says which is possible rather than asserting one.
    UnexpectedCompletion {
        /// The identity the unrecognised completion carried.
        user_data: usize,
    },
    /// Two completions arrived for the same `user_data`.
    ///
    /// The "exactly" half of "exactly one completion". Counted separately from
    /// [`Violation::UnexpectedCompletion`] because a duplicate is a much
    /// stronger signal than an unrecognised identity: it cannot be explained
    /// by a caller forgetting to report a push.
    DuplicateCompletion {
        /// The identity that completed more than once.
        user_data: usize,
    },
    /// An operation was pushed and never completed.
    Outstanding {
        /// The identity that was pushed and never completed.
        user_data: usize,
    },
    /// A token was dropped without being claimed, and not deliberately.
    ///
    /// `Token` leaks on an unclaimed drop, which keeps the kernel's pointer
    /// valid and is correct. It also means whatever that token held --
    /// a buffer, a registered-buffer use count, a file guard -- is gone for
    /// the process's life. A caller that means to do this says so with
    /// [`RingContract::observe_deliberate_leak`].
    LeakedToken {
        /// The identity whose token was dropped unclaimed.
        user_data: usize,
    },
    /// A registered buffer still had operations outstanding at quiescence.
    BufferStillInUse {
        /// Position of the buffer within its registration.
        index: u32,
        /// How many operations it still had in flight.
        outstanding: usize,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedCompletion { user_data } => write!(
                f,
                "completion for user_data {user_data:#x} matches no observed push: either it \
                 completed twice, or a push was not reported to the contract"
            ),
            Self::DuplicateCompletion { user_data } => write!(
                f,
                "user_data {user_data:#x} completed more than once, but every queued SQE \
                 produces exactly one completion"
            ),
            Self::Outstanding { user_data } => {
                write!(f, "user_data {user_data:#x} was pushed and never completed")
            }
            Self::LeakedToken { user_data } => write!(
                f,
                "the token for user_data {user_data:#x} was dropped unclaimed, leaking whatever \
                 it held for the life of the process"
            ),
            Self::BufferStillInUse { index, outstanding } => write!(
                f,
                "registered buffer {index} still has {outstanding} operation(s) outstanding at \
                 quiescence"
            ),
        }
    }
}

/// What one observed operation is known to have done so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Queued, no completion seen.
    Pushed,
    /// Completed exactly once, and its token claimed.
    Completed,
    /// Completed, but its token was dropped unclaimed.
    Leaked,
    /// Deliberately abandoned by a caller who said so.
    DeliberatelyLeaked,
}

/// An executable form of this crate's conservation rules.
///
/// Fed by the caller rather than wired into [`crate::Batch`], for two reasons.
/// A ring can be driven through `push_raw` and the `_raw` entry points without
/// any of this crate's bookkeeping being involved at all, so an internal hook
/// would silently cover less than it appears to. And a consumer validating its
/// own harness needs to drive the same rules from outside.
///
/// # Example
///
/// ```
/// use windows_ioring_sys::contract::RingContract;
///
/// let mut contract = RingContract::new();
/// contract.observe_push(0x1234);
/// contract.observe_completion(0x1234);
/// contract.observe_claim(0x1234);
/// assert!(contract.check_quiescent().is_empty());
/// ```
#[derive(Debug, Default)]
pub struct RingContract {
    operations: HashMap<usize, State>,
    /// Per registered-buffer outstanding counts, as the caller reports them.
    buffers: HashMap<u32, usize>,
    violations: Vec<Violation>,
}

impl RingContract {
    /// A contract with nothing observed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that an SQE was successfully queued with this `user_data`.
    ///
    /// Report only pushes that **queued**. A `Build*` call that failed
    /// synchronously releases its reservation and produces no completion, so
    /// reporting it would manufacture an [`Violation::Outstanding`] at
    /// teardown.
    pub fn observe_push(&mut self, user_data: usize) {
        self.operations.insert(user_data, State::Pushed);
    }

    /// Record a completion popped from the ring.
    pub fn observe_completion(&mut self, user_data: usize) {
        match self.operations.get(&user_data) {
            None => self
                .violations
                .push(Violation::UnexpectedCompletion { user_data }),
            Some(State::Pushed) => {
                // Provisionally leaked: a completion whose token is never
                // claimed *is* a leak, so this is corrected by
                // `observe_claim` rather than assumed benign.
                self.operations.insert(user_data, State::Leaked);
            }
            Some(_) => self
                .violations
                .push(Violation::DuplicateCompletion { user_data }),
        }
    }

    /// Record that the operation's token was claimed against its completion,
    /// returning whatever it held.
    pub fn observe_claim(&mut self, user_data: usize) {
        if let Some(state) = self.operations.get_mut(&user_data) {
            *state = State::Completed;
        }
    }

    /// Record that a token was abandoned on purpose.
    ///
    /// Leaking is a legitimate choice -- it is what keeps a buffer alive when
    /// a caller cannot prove the kernel is finished with it -- so it is
    /// excused when it is *stated*. An unstated leak is still reported,
    /// because the difference between the two is exactly the bug worth
    /// finding.
    pub fn observe_deliberate_leak(&mut self, user_data: usize) {
        self.operations.insert(user_data, State::DeliberatelyLeaked);
    }

    /// Record a registered buffer's outstanding count, as
    /// [`crate::RegisteredBuffers::outstanding`] reports it.
    pub fn observe_buffer(&mut self, index: u32, outstanding: usize) {
        self.buffers.insert(index, outstanding);
    }

    /// Violations seen so far, without asking about quiescence.
    #[must_use]
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// Every violation, including operations still outstanding and buffers
    /// still in use.
    ///
    /// Call once everything is expected to have finished. An empty result is
    /// the claim that nothing was lost.
    #[must_use]
    pub fn check_quiescent(&self) -> Vec<Violation> {
        let mut all = self.violations.clone();

        // Sorted so a failure reads the same way twice. `HashMap` iteration
        // order is deliberately unspecified, and an oracle whose output
        // reorders between runs is one nobody can diff.
        let mut pending: Vec<_> = self
            .operations
            .iter()
            .filter_map(|(user_data, state)| match state {
                State::Pushed => Some(Violation::Outstanding {
                    user_data: *user_data,
                }),
                State::Leaked => Some(Violation::LeakedToken {
                    user_data: *user_data,
                }),
                State::Completed | State::DeliberatelyLeaked => None,
            })
            .collect();
        pending.sort_by_key(|violation| match violation {
            Violation::Outstanding { user_data } | Violation::LeakedToken { user_data } => {
                *user_data
            }
            _ => 0,
        });
        all.extend(pending);

        let mut busy: Vec<_> = self
            .buffers
            .iter()
            .filter(|(_, outstanding)| **outstanding > 0)
            .map(|(index, outstanding)| Violation::BufferStillInUse {
                index: *index,
                outstanding: *outstanding,
            })
            .collect();
        busy.sort_by_key(|violation| match violation {
            Violation::BufferStillInUse { index, .. } => *index,
            _ => 0,
        });
        all.extend(busy);

        all
    }

    /// Panic with every violation if the contract was broken.
    ///
    /// The convenience form for a test. Reports **all** violations rather than
    /// the first, because they are usually one cause and seeing only one of
    /// them invites fixing a symptom.
    ///
    /// # Panics
    ///
    /// If [`RingContract::check_quiescent`] finds anything.
    pub fn assert_quiescent(&self) {
        let violations = self.check_quiescent();
        assert!(
            violations.is_empty(),
            "the ring contract was broken:\n{}",
            violations
                .iter()
                .map(|violation| format!("  - {violation}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[cfg(test)]
mod tests;
