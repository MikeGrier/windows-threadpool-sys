// Copyright (c) 2026 Mike Grier
//! [`RingContract`](crate::contract::RingContract) -- this crate's conservation
//! rules, made executable.
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
//!
//! Not every push carries a token: the `_raw` flush and cancel entry points
//! return a bare `user_data`, because they own nothing a claim could hand
//! back. Report those with
//! [`RingContract::observe_push`](crate::contract::RingContract::observe_push),
//! or the oracle will demand a claim that cannot be made.
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
//!   There is one promised ordering -- a covering flush against the operations
//!   *preceding* it, and nothing about those following it
//!   ([D-24](../DESIGN-NOTES.md#d-24), as corrected by
//!   [D-47](../DESIGN-NOTES.md#d-47-detail)) -- and this oracle does not check
//!   it. Not because it is unobservable: tag the operations and it is entirely
//!   observable from the completion stream, which is exactly how
//!   [tests/flush_barrier.rs](../tests/flush_barrier.rs) tests it and how D-47
//!   measured the flag's one-sidedness over some 4,500 trials. It is that
//!   *this* oracle is a conservation checker -- it matches each completion
//!   against an outstanding claim and never relates that claim to the order
//!   anything was submitted in, so it holds nothing it could compare against.
//!
//!   An observer that does want to check it needs tagged identities, so that a
//!   popped completion can be attributed to the operation it belongs to. Pop
//!   order itself is a faithful witness of the order the kernel *posted* those
//!   completions -- that is precisely the observable D-47 measured, and what
//!   this crate's contract is stated in terms of. What it does not witness is
//!   the order the operations *began executing*, which is invisible from user
//!   mode -- see [D-47](../DESIGN-NOTES.md#d-47-detail) for why the contract is
//!   stated in terms of completion order and stops there.
//! - **Whether an operation succeeded.** A failed operation still completes
//!   exactly once; conservation and success are different questions.
//! - **Anything about the device.** Whether a flush truly reached durable
//!   media is invisible here and everywhere else short of a power cut.
//! - **That a caller pushed anything sensible.** This checks conservation of
//!   what was pushed, not that pushing it was a good idea.

use std::collections::{HashMap, HashSet, VecDeque};
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
            Self::BufferStillInUse { index, outstanding } => write!(
                f,
                "registered buffer {index} still has {outstanding} operation(s) outstanding at \
                 quiescence"
            ),
        }
    }
}

/// What one operation *still being tracked* is known to have done so far.
///
/// Terminal outcomes are deliberately absent. An operation that finishes --
/// which since [D-74](../DESIGN-NOTES.md#d-74) means simply that its
/// completion was popped -- leaves this map entirely and its identity moves
/// to the bounded history described
/// on [`RingContract`], because retaining a terminal entry per operation is
/// what made the oracle grow without limit (M28.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Queued, no completion seen.
    ///
    /// **One push state, as of `D-74`.** There were two, because a `_raw`
    /// push returned a bare `user_data` while the others returned a
    /// `Token`, and conflating them reported a leak for every
    /// tokenless push -- a violation the caller could not satisfy, since
    /// there was no token to claim. With the ring holding what an operation
    /// carries, no push hands the caller anything to lose, so the distinction
    /// has nothing left to distinguish.
    Pushed,
}

/// How many finished identities the oracle remembers for duplicate diagnosis.
///
/// A **diagnostic window, not a correctness parameter.** Nothing is missed
/// when a duplicate falls outside it: the completion is still reported, as
/// [`Violation::UnexpectedCompletion`] rather than
/// [`Violation::DuplicateCompletion`]. What the window buys is the *stronger*
/// of the two claims, which the variants' own docs explain is worth keeping --
/// a duplicate cannot be explained away by a caller forgetting to report a
/// push, and an unrecognised identity can.
///
/// Changing it is not a breaking change. It trades a fixed amount of memory
/// for how far back a duplicate is still named precisely.
const FINISHED_HISTORY: usize = 1024;

/// An executable form of this crate's conservation rules.
///
/// Fed by the caller rather than wired into [`crate::Batch`], for two reasons.
/// A ring can be driven through `push_raw` and the `_raw` entry points without
/// any of this crate's bookkeeping being involved at all, so an internal hook
/// would silently cover less than it appears to. And a consumer validating its
/// own harness needs to drive the same rules from outside.
///
/// # What this costs to run, which is bounded (M28.2)
///
/// Memory here is a function of **operations in flight**, not of operations
/// ever performed. An operation that finishes leaves the tracking map, so a
/// consumer that pushes and claims forever holds a map whose size follows its
/// own concurrency rather than its uptime.
///
/// That was not true before M28.2: a claimed operation was marked `Completed`
/// and kept, so the oracle retained one entry per operation for the life of
/// the process. The sample never showed it because it appends 24 records, and
/// nothing documented it -- so a long-running consumer following this crate's
/// own recommendation grew without limit. It is also why an always-on checked
/// inventory was ruled out in `M23.3`, since a check that cannot be left
/// running is not a check.
///
/// Retiring an entry would lose the ability to call a later completion for it
/// a *duplicate* rather than merely unrecognised, so the last
/// [`FINISHED_HISTORY`] finished identities are remembered for that purpose
/// alone. Beyond that window a duplicate is still reported, under the weaker
/// name.
///
/// # Example
///
/// ```
/// use windows_ioring_sys::contract::RingContract;
///
/// let mut contract = RingContract::new();
/// contract.observe_push(0x1234);
/// contract.observe_completion(0x1234);
/// assert!(contract.check_quiescent().is_empty());
/// ```
#[derive(Debug, Default)]
pub struct RingContract {
    /// Operations still being tracked. Terminal outcomes are not kept here;
    /// see the type's own documentation for why.
    operations: HashMap<usize, State>,
    /// Identities that finished, most recent last, capped at
    /// [`FINISHED_HISTORY`]. Paired with `finished_set` so the membership test
    /// stays O(1) -- the queue exists only to know which one to evict.
    finished: VecDeque<usize>,
    /// Membership index over `finished`, held in step with it.
    finished_set: HashSet<usize>,
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
    /// There is one push observer, as of `M28.5`. There were two:
    /// `observe_push` recorded an SQE that carried no token to
    /// claim -- the `_raw` flush and cancel entry points -- because reporting
    /// one as an ordinary push produced a leak violation its caller had no
    /// way to satisfy, there being no token to claim. [D-74](../DESIGN-NOTES.md#d-74)
    /// removed the leak rule, which made the two identical in body as well as
    /// in purpose, and two names for one behaviour is a restatement waiting
    /// for the next change to reach only one of them.
    pub fn observe_push(&mut self, user_data: usize) {
        self.operations.insert(user_data, State::Pushed);
    }

    /// Record a completion popped from the ring.
    pub fn observe_completion(&mut self, user_data: usize) {
        match self.operations.get(&user_data) {
            None => {
                // Not tracked. Either it finished already -- which the
                // history can still prove, and which is the stronger claim --
                // or no observed push ever produced it.
                if self.finished_set.contains(&user_data) {
                    self.violations
                        .push(Violation::DuplicateCompletion { user_data });
                } else {
                    self.violations
                        .push(Violation::UnexpectedCompletion { user_data });
                }
            }
            Some(State::Pushed) => {
                // Terminal. A completion used to be *provisional* here,
                // parked in a leaked state until `observe_claim` corrected
                // it, because a completion whose token was never claimed was
                // a real leak. The pop is the claim now (`D-74`), so there is
                // no second step for the caller to omit and nothing for this
                // to wait on.
                self.retire(user_data);
            }
        }
    }

    /// Retire a finished identity: out of the tracking map, into the bounded
    /// history.
    ///
    /// This is the one place an operation stops costing memory proportional to
    /// how many have run, which is what `M28.2` was about.
    fn retire(&mut self, user_data: usize) {
        self.operations.remove(&user_data);
        if self.finished_set.insert(user_data) {
            self.finished.push_back(user_data);
            if self.finished.len() > FINISHED_HISTORY
                && let Some(evicted) = self.finished.pop_front()
            {
                self.finished_set.remove(&evicted);
            }
        }
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
            // Not a `filter_map`: since `M28.2` retired the terminal states,
            // every entry still tracked is a violation at quiescence.
            .map(|(user_data, state)| match state {
                State::Pushed => Violation::Outstanding {
                    user_data: *user_data,
                },
            })
            .collect();
        pending.sort_by_key(|violation| match violation {
            Violation::Outstanding { user_data } => *user_data,
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
