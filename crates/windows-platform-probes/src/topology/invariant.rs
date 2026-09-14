// Copyright (c) Mike Grier.

//! States an observation can be in that forbid an agreeing verdict.
//!
//! # These are the correspondences that survived, moved off the text
//!
//! The report oracle (`crate::report_oracle`, present only in builds that run
//! it, so deliberately not a link from here) once checked relations between a
//! report's two rendered
//! halves. The rules worth keeping are not really about rendering -- they relate
//! a STATE to the verdict, and the report is only where that relation became
//! visible. They are here instead, as predicates over [`Observation`], per
//! [DESIGN-NOTES.md](../../DESIGN-NOTES.md#d-encoded-row-is-the-contract).
//!
//! Two things change by moving them. They run whether or not anything was
//! rendered, so a caller that measures and never builds a report still gets
//! them; and no parser stands between the rule and the values it reads, which is
//! where a large share of this crate's defects lived.
//!
//! # Why every rule here reads the OBSERVATION, and none reads the lists
//!
//! This was got wrong on the first attempt, and the reason is worth recording
//! because it is not obvious.
//!
//! [`CrossCheck::verdict`] is a pure function of the three lists: a non-empty
//! `disagreements` gives `Disagree`, two empty lists beside it give `Agree`,
//! anything else gives `Incomplete`. So a rule of the form "a non-empty
//! `parse_incomplete` forbids `agree`" is not an invariant at all -- it restates
//! the definition, cannot fail for any input, and three such rules were written
//! here before that was noticed.
//!
//! Worse than useless: such a rule **cannot catch the defect this component
//! exists because of.** That defect was a state -- a named partitioning level
//! with no summary -- that `cross_check` had no branch for. A missing branch
//! means the list stays EMPTY, so a list-reading rule sees nothing to complain
//! about and the verdict it produces is `Agree` legitimately. The evidence that
//! something is wrong survives only in the observation.
//!
//! So each rule below names a state, computes it from the observation, and
//! requires the verdict to have moved off `agree`. A push site deleted from
//! `cross_check` leaves the state visible here and fires the rule; that is the
//! whole design, and it is what the sabotage evidence in
//! [COMPLETED-CHECKLIST.md](../../COMPLETED-CHECKLIST.md) M3.2 demonstrates. It
//! is also the manifest entry `cross_check forgets the changed bracket` in
//! [sabotage.json](../../sabotage.json), which re-runs that evidence rather than
//! leaving it as a claim about a sabotage somebody once performed.
//!
//! # What that means for testing them
//!
//! No input can violate these while `cross_check` is correct, because
//! `cross_check` is what makes them hold. They are reachable by CODE CHANGE, not
//! by data -- so [`check`] takes the verdict alongside the observation, letting
//! a test supply the answer a broken `cross_check` would give, and the sabotage
//! loop confirms that a real deletion reddens a real test.

use std::fmt;

use super::{Coherence, CrossCheck, Observation, PartitioningCache, Verdict};

#[cfg(test)]
mod tests;

/// Declares [`BlockingState`]: the variants, [`BlockingState::ALL`] and
/// [`BlockingState::described`] all from ONE list.
///
/// **This exists so that `ALL` cannot drift from the enum.** Writing the two by
/// hand does not prevent it, and the difference is not cosmetic: `ALL` is what
/// the completeness guard iterates, so a variant missing from it is a blocking
/// state nothing tests. A hand-written `ALL` was measured to allow exactly that
/// -- a new variant, its `described()` arm supplied because the `match` forces
/// one, compiled cleanly and left all ten invariant tests green while being
/// reached by none of them.
///
/// The `match` in `described()` is genuinely exhaustive-checked, which is what
/// made the hand-written version look safe. It is not enough: it forces a new
/// variant to acquire an ARM, never an ENTRY in a separate array. Generating
/// both from one list is what ties them together, because the enum itself comes
/// from that list -- a variant that is not in it does not exist.
macro_rules! blocking_states {
    ($( $(#[$doc:meta])* $variant:ident => $described:literal ),+ $(,)?) => {
        /// A state an observation can be in that forbids an agreeing verdict.
        ///
        /// **A type rather than a `&'static str`, so completeness is
        /// checkable.** These were strings, and the test that claimed to check
        /// every state had a perturbation derived BOTH of its sets from the
        /// perturbation table -- so a new branch in [`blocking_states`] that no
        /// mutation reached appeared in neither set and both loops stayed green.
        /// The guard could only confirm that existing labels described existing
        /// mutations.
        ///
        /// The type is declared by a macro from a single list, so
        /// [`BlockingState::ALL`] cannot omit a variant: the variants and `ALL`
        /// are the same list. An earlier version wrote them separately and
        /// claimed the compiler checked the correspondence, which it did not --
        /// found by a review, two rounds after the strings.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum BlockingState {
            $( $(#[$doc])* $variant, )+
        }

        impl BlockingState {
            /// Every state, so a test can check the perturbation table covers
            /// them all.
            ///
            /// Generated from the same list as the variants, so it is exhaustive
            /// by construction rather than by anyone remembering.
            pub const ALL: &'static [Self] = &[ $( Self::$variant, )+ ];

            /// How the report names this state, for a violation a reader has to
            /// act on.
            #[must_use]
            pub const fn described(self) -> &'static str {
                match self {
                    $( Self::$variant => $described, )+
                }
            }
        }
    };
}

blocking_states! {
    /// A level was named as the outermost partitioning cache with no summary.
    ///
    /// The state the renderer prints as `BUG IN THIS PROBE`, and the defect this
    /// component exists because of.
    PartitioningSummaryMissing => "summary missing for the outermost partitioning cache",
    /// The enumeration recorded anomalies.
    EnumerationAnomalies => "the enumeration recorded anomalies",
    /// The topology was not measured from a running machine.
    NotMeasured => "the topology was not measured from a running machine",
    /// No cache levels were reported.
    NoCacheLevels => "no cache levels were reported",
    /// No packages were reported, though the machine has one.
    NoPackages => "no packages were reported",
    /// No cores were reported, though the machine has one.
    NoCores => "no cores were reported",
    /// A core record contradicts itself.
    ContradictoryCore => "a core record contradicts itself",
    /// A cache level is numbered 0, which Windows does not report.
    UnnumberedCacheLevel => "a cache level is numbered 0",
    /// The crate's two enumerations did not agree.
    EnumerationsDisagreed => "the crate's two enumerations did not agree",
    /// Coherence between the two enumerations was never established.
    ///
    /// **Distinct from `EnumerationsDisagreed`, and the separation is load
    /// bearing.** One branch covered both, on the reading that anything other
    /// than `Agreed` forbids agreement -- true of the VERDICT and false of the
    /// ROW, which publishes `coherence_not_collected` here and
    /// `enumerations_disagreed` there. A state that names the wrong code makes
    /// the per-state publication rule demand something the renderer never emits.
    /// Latent until a corpus shape reached it. Found by a review.
    CoherenceNotCollected => "coherence between the two enumerations was not collected",
    /// The bracket did not establish that the machine held still.
    BracketNotHeld => "the bracket did not establish that the machine held still",
}

/// A state that forbids an agreeing verdict, found beside one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// The observation is in a state that must prevent `agree`, and did not.
    StateWithAgreeingVerdict {
        /// The state.
        state: BlockingState,
    },
    /// `agree` without the comparison it asserts having been made.
    ///
    /// `agree` is the claim that every check this probe could make WAS made and
    /// matched, so a counter that reported failure cannot sit beside it.
    AgreedWithoutComparingCounter {
        /// The counter that was not compared.
        counter: &'static str,
    },
    /// `agree` beside a counter that does not equal what was enumerated.
    AgreedDespiteCounterMismatch {
        /// The counter.
        counter: &'static str,
        /// What the parse carried.
        parsed: usize,
        /// What the counter reported.
        read: usize,
    },
    /// `agree` beside a NUMA highest-node number that does not match the parse.
    ///
    /// Separate from [`Violation::AgreedDespiteCounterMismatch`] because the
    /// quantity is different in kind: a largest node NUMBER, optional on both
    /// sides, rather than a count. Folding it into the count-shaped variant
    /// would have meant inventing a `usize` for an absent parse.
    AgreedDespiteNumaMismatch {
        /// What the parse carried, which may be nothing.
        parsed: Option<u32>,
        /// What `GetNumaHighestNodeNumber` reported.
        counter: u32,
    },
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateWithAgreeingVerdict { state } => write!(
                f,
                "the observation is in the `{}` state, which forbids an \
                 agreeing verdict, but the verdict is `agree` -- so whatever in \
                 `cross_check` should have reported this state did not",
                state.described(),
            ),
            Self::AgreedWithoutComparingCounter { counter } => write!(
                f,
                "the verdict is `agree`, which asserts every check was made, \
                 but {counter} reported failure so its comparison was not made"
            ),
            Self::AgreedDespiteCounterMismatch {
                counter,
                parsed,
                read,
            } => write!(
                f,
                "the verdict is `agree` but the parse carries {parsed} where \
                 {counter} read {read}"
            ),
            Self::AgreedDespiteNumaMismatch { parsed, counter } => match parsed {
                Some(parsed) => write!(
                    f,
                    "the verdict is `agree` but the parse's highest NUMA node is \
                     {parsed} where GetNumaHighestNodeNumber read {counter}"
                ),
                None => write!(
                    f,
                    "the verdict is `agree` but the parse reports no NUMA node at \
                     all where GetNumaHighestNodeNumber read {counter}"
                ),
            },
        }
    }
}

/// Every state `observation` is in that forbids an agreeing verdict.
///
/// **Computed from the observation, never from the cross-check's lists.** Each
/// of these has a push site in [`Observation::cross_check`], and the point of
/// enumerating them separately is that a deleted push site leaves the state
/// here and the list empty -- so this is what notices, and the list could not.
///
/// Not every entry in `parse_incomplete` appears here, and that is deliberate
/// rather than an omission: some are derived counts whose only source IS the
/// cross-check's own arithmetic, so restating them would be the tautology this
/// module exists to avoid. What belongs here is a state readable from the
/// observation on its own terms.
#[must_use]
pub fn blocking_states(observation: &Observation) -> Vec<BlockingState> {
    let mut states = Vec::new();

    // The defect this component exists because of: the renderer prints this
    // state as `BUG IN THIS PROBE ... Nothing below about cache partitioning
    // can be trusted`, and `cross_check` had no branch for it.
    if matches!(
        observation.partitioning_cache(),
        PartitioningCache::SummaryMissing(_)
    ) {
        states.push(BlockingState::PartitioningSummaryMissing);
    }

    if !observation.enumeration_anomalies.is_empty() {
        states.push(BlockingState::EnumerationAnomalies);
    }

    if !observation.topology_was_measured {
        states.push(BlockingState::NotMeasured);
    }

    if observation.caches.is_empty() {
        states.push(BlockingState::NoCacheLevels);
    }

    if observation.online_processors > 0 && observation.packages == 0 {
        states.push(BlockingState::NoPackages);
    }

    if observation.online_processors > 0 && observation.cores.is_empty() {
        states.push(BlockingState::NoCores);
    }

    if observation
        .cores
        .iter()
        .any(super::CoreShape::contradicts_itself)
    {
        states.push(BlockingState::ContradictoryCore);
    }

    if observation.caches.iter().any(|cache| cache.level == 0) {
        states.push(BlockingState::UnnumberedCacheLevel);
    }

    // **Matched by variant rather than by `!= Agreed`**, because `cross_check`
    // files these under different codes and a state must name the code its own
    // condition emits. `Coherence` is not `#[non_exhaustive]`, so this match is
    // compiler-exhaustive and a fourth variant cannot be silently folded into
    // whichever arm happens to be nearest -- which is what the `!= Agreed` form
    // did to `NotCollected`.
    match observation.coherence {
        Coherence::Agreed => {}
        Coherence::Disagreed { .. } => states.push(BlockingState::EnumerationsDisagreed),
        Coherence::NotCollected => states.push(BlockingState::CoherenceNotCollected),
    }

    if observation.bracket != super::BracketOutcome::HeldStill {
        states.push(BlockingState::BracketNotHeld);
    }

    states
}

/// Every invariant relating `observation` to `verdict` that does not hold.
///
/// Empty is the answer for every pair this crate can produce, because
/// `cross_check` is what makes these hold. A non-empty result means a push site
/// or a guard in `cross_check` stopped reporting a state the observation still
/// shows.
///
/// **Takes the verdict rather than deriving it**, so a test can supply the
/// answer a broken `cross_check` would give and see the rule fire. Deriving it
/// would leave every branch reachable only by editing the source, and a green
/// run would carry no information about whether the branch works.
#[must_use]
pub fn check(observation: &Observation, verdict: Verdict) -> Vec<Violation> {
    if verdict != Verdict::Agree {
        return Vec::new();
    }

    let mut found: Vec<Violation> = blocking_states(observation)
        .into_iter()
        .map(|state| Violation::StateWithAgreeingVerdict { state })
        .collect();

    // Zero is how both counters report failure, so a zero beside `agree` is the
    // verdict claiming a comparison that could not have happened.
    //
    // **The NUMA counter is held too, one rule further down, and the reason it
    // was once absent is worth keeping because it was half right.** It reports
    // the largest node NUMBER rather than a count, so there is no enumerated
    // quantity to hold it to -- nodes 0 and 2 are a valid sparse topology, and
    // comparing it against `numa_domains` would manufacture a violation on
    // hardware reporting itself correctly. That argument rules out ONE
    // comparison. It does not rule out the comparison `cross_check` actually
    // makes, which is highest-against-highest, and excluding NUMA from here on
    // the strength of it left the module's own claim -- that a push site deleted
    // from `cross_check` fires a rule here -- false for precisely those two push
    // sites. Found by a review.
    for (counter, parsed, read) in [
        (
            "GetActiveProcessorCount",
            observation.online_processors,
            observation.raw_active_processors as usize,
        ),
        (
            "GetActiveProcessorGroupCount",
            observation.groups,
            observation.raw_group_count as usize,
        ),
    ] {
        if read == 0 {
            found.push(Violation::AgreedWithoutComparingCounter { counter });
        } else if parsed != read {
            found.push(Violation::AgreedDespiteCounterMismatch {
                counter,
                parsed,
                read,
            });
        }
    }

    // The NUMA comparison, in the shape `cross_check` makes it: highest node
    // number against highest node number, never against a count. `agree` is
    // reachable only through both of that function's NUMA branches declining to
    // fire, so beside `agree` the counter must have been readable AND equal.
    match observation.raw_highest_numa_node {
        None => found.push(Violation::AgreedWithoutComparingCounter {
            counter: "GetNumaHighestNodeNumber",
        }),
        Some(counter) if observation.highest_numa_node != Some(counter) => {
            found.push(Violation::AgreedDespiteNumaMismatch {
                parsed: observation.highest_numa_node,
                counter,
            });
        }
        Some(_) => {}
    }

    found
}

/// [`check`], as an assertion, for the call sites that are bound to it.
///
/// **Replacing this body with `()` survives a mutation sweep, and no test can
/// change that.** Recorded here rather than left for the next sweep to
/// re-discover, because the argument is short and the alternative is a test
/// manufactured to reach code nothing can reach.
///
/// It derives the verdict from `observation.cross_check()`, so the pair it
/// checks is always the pair the crate itself produces -- and [`check`]'s rules
/// hold for every such pair by construction, as the module header explains: they
/// are reachable by CODE CHANGE, not by data. That is precisely why [`check`]
/// takes the verdict as a PARAMETER, letting a test supply the answer a broken
/// `cross_check` would give; this function has no such seam, so there is no
/// observation for which it panics and nothing to distinguish it from `()`.
///
/// The same survivor was recorded for `assert_row_is_well_formed` on PR #88 -- see
/// [DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md) -- for the same reason: every
/// instrument that would notice goes THROUGH it. A binding that cannot fail on
/// data is checked by the sweep's `caught` results on [`check`] itself, which is
/// where the behaviour lives.
///
/// # Panics
///
/// Panics listing every invariant the observation violated.
pub fn assert_holds(observation: &Observation) {
    let violations = check(observation, observation.cross_check().verdict());
    assert!(
        violations.is_empty(),
        "an observation violated {} invariant(s) relating it to its verdict:\n{}",
        violations.len(),
        violations
            .iter()
            .map(|violation| format!("  - {violation}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// Named so the module's own import of [`CrossCheck`] is not dead.
///
/// `assert_holds` reaches the verdict through it, and a reader looking for the
/// relation between the two types should find it stated rather than inferred.
const _: fn(&CrossCheck) -> Verdict = CrossCheck::verdict;
