// Copyright (c) Mike Grier.

//! States an observation can be in that forbid an agreeing verdict.
//!
//! # These are the correspondences that survived, moved off the text
//!
//! [`crate::report_oracle`] checks relations between a report's two rendered
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
//! [CHECKLIST.md](../../CHECKLIST.md) M3.2 demonstrates.
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

/// A state that forbids an agreeing verdict, found beside one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// The observation is in a state that must prevent `agree`, and did not.
    StateWithAgreeingVerdict {
        /// The state, as [`blocking_states`] names it.
        state: &'static str,
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
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateWithAgreeingVerdict { state } => write!(
                f,
                "the observation is in the `{state}` state, which forbids an \
                 agreeing verdict, but the verdict is `agree` -- so whatever in \
                 `cross_check` should have reported this state did not"
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
pub fn blocking_states(observation: &Observation) -> Vec<&'static str> {
    let mut states = Vec::new();

    // The defect this component exists because of: the renderer prints this
    // state as `BUG IN THIS PROBE ... Nothing below about cache partitioning
    // can be trusted`, and `cross_check` had no branch for it.
    if matches!(
        observation.partitioning_cache(),
        PartitioningCache::SummaryMissing(_)
    ) {
        states.push("summary missing for the outermost partitioning cache");
    }

    if !observation.enumeration_anomalies.is_empty() {
        states.push("the enumeration recorded anomalies");
    }

    if !observation.topology_was_measured {
        states.push("the topology was not measured from a running machine");
    }

    if observation.caches.is_empty() {
        states.push("no cache levels were reported");
    }

    if observation.online_processors > 0 && observation.packages == 0 {
        states.push("no packages were reported");
    }

    if observation.online_processors > 0 && observation.cores.is_empty() {
        states.push("no cores were reported");
    }

    if observation
        .cores
        .iter()
        .any(super::CoreShape::contradicts_itself)
    {
        states.push("a core record contradicts itself");
    }

    if observation.caches.iter().any(|cache| cache.level == 0) {
        states.push("a cache level is numbered 0");
    }

    if !matches!(observation.coherence, Coherence::Agreed) {
        states.push("the crate's two enumerations did not agree");
    }

    if observation.bracket != super::BracketOutcome::HeldStill {
        states.push("the bracket did not establish that the machine held still");
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
    // verdict claiming a comparison that could not have happened. The NUMA
    // counter is deliberately absent, for the reason the renderer gives: it
    // reports the largest node NUMBER rather than a count, so there is no
    // enumerated quantity to hold it to, and nodes 0 and 2 are a valid sparse
    // topology.
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

    found
}

/// [`check`], as an assertion, for the call sites that are bound to it.
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
