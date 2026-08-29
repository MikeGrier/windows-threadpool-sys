// Copyright (c) 2026 Mike Grier
//! Replay and verification (M13.5).
//!
//! Everything before this file *demonstrates* the pattern. This is the only
//! part that can catch a durability bug, because it is the only part that
//! checks the claim [`crate::contract`] actually makes rather than the steps
//! taken to reach it.
//!
//! # What it checks, clause by clause
//!
//! The contract's guarantee is that **a record is durable when the commit of
//! the epoch containing it has completed**, so replay is handed the watermark
//! the log reported and holds it to exactly that:
//!
//! - Every record belonging to an epoch at or below the watermark **must** be
//!   present, in sequence, with a payload that matches and a checksum that
//!   validates. A gap, a tear, or a mismatch there is a durability bug -- the
//!   log reported something durable that is not.
//! - Records belonging to epochs **above** the watermark may be wholly
//!   present, wholly absent, or torn. All three are legal outcomes of the same
//!   crash, so replay counts them and asserts nothing about them. Refusing to
//!   tolerate a torn tail would be its own bug: the contract promises the tail
//!   is unreliable, and a reader that treated that as corruption would reject
//!   a perfectly healthy log.
//!
//! That asymmetry is the whole design. The durable region is held to a strict
//! standard; the tail is held to none.
//!
//! # Why this is evidence and not decoration
//!
//! A verifier that cannot fail proves nothing, so the sample runs it against a
//! deliberately damaged log too (see `main`'s negative control). If corrupting
//! a byte inside the durable region does not produce a
//! [`Violation`], this file is not checking what it claims to check.

use crate::commit::Epoch;
use crate::record::{self, Sequence, Torn};

/// A way the log failed the contract.
///
/// Every variant here means the log reported something durable that replay
/// could not find intact, which is the failure the whole sample exists to be
/// able to detect.
#[derive(Debug, PartialEq, Eq)]
pub enum Violation {
    /// A record in a committed epoch could not be read back.
    MissingDurableRecord { sequence: u64, reason: Torn },
    /// A record in a committed epoch read back with the wrong payload.
    WrongPayload { sequence: u64 },
    /// A record in a committed epoch carried an unexpected sequence, so the
    /// log is not the ordered stream it claims to be.
    WrongSequence { expected: u64, found: u64 },
    /// A record claimed an epoch above the watermark but appeared before one
    /// that did not, so the on-disk epoch stamps are not monotonic.
    EpochWentBackwards { sequence: u64, epoch: u64 },
}

/// What one replay found.
#[derive(Debug)]
pub struct Outcome {
    /// Records at or below the watermark, verified intact and in order.
    pub durable_verified: usize,
    /// Whole records found above the watermark. Not a guarantee of anything --
    /// the contract promises nothing about them -- just what was there.
    pub tail_records: usize,
    /// Why decoding stopped, if it stopped before the end of the file. Past
    /// the watermark this is expected rather than exceptional.
    pub tail_stopped: Option<Torn>,
    /// Contract failures. Empty means the log kept every promise it made.
    pub violations: Vec<Violation>,
}

impl Outcome {
    /// Whether the log honoured its contract.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Read `bytes` back as a log and check it against the contract.
///
/// `durable_through` is the watermark the log reported, and
/// `expected_durable` how many records it reported durable. `payload_for`
/// reproduces what record *n* should contain, so a payload that came back
/// altered is caught rather than merely present.
pub fn replay(
    bytes: &[u8],
    durable_through: Epoch,
    expected_durable: usize,
    payload_for: impl Fn(usize) -> Vec<u8>,
) -> Outcome {
    let mut outcome = Outcome {
        durable_verified: 0,
        tail_records: 0,
        tail_stopped: None,
        violations: Vec::new(),
    };

    let mut cursor = 0;
    let mut next_sequence = 0_u64;
    let mut past_watermark = false;

    while cursor < bytes.len() {
        match record::decode(&bytes[cursor..]) {
            Ok(found) => {
                cursor += found.total_len;

                if found.epoch > durable_through {
                    // The tail. Nothing is promised about it, so nothing is
                    // asserted -- only counted.
                    past_watermark = true;
                    outcome.tail_records += 1;
                    next_sequence = found.sequence.0 + 1;
                    continue;
                }

                // A durable-region record appearing *after* the tail started
                // would mean the epoch stamps are not monotonic, which would
                // make the watermark meaningless.
                if past_watermark {
                    outcome.violations.push(Violation::EpochWentBackwards {
                        sequence: found.sequence.0,
                        epoch: found.epoch.0,
                    });
                    continue;
                }

                if found.sequence != Sequence(next_sequence) {
                    outcome.violations.push(Violation::WrongSequence {
                        expected: next_sequence,
                        found: found.sequence.0,
                    });
                }
                if found.payload != payload_for(found.sequence.0 as usize) {
                    outcome.violations.push(Violation::WrongPayload {
                        sequence: found.sequence.0,
                    });
                }
                outcome.durable_verified += 1;
                next_sequence = found.sequence.0 + 1;
            }
            Err(reason) => {
                outcome.tail_stopped = Some(reason);
                // Stopping *inside* the durable region is the failure this
                // whole sample exists to be able to detect: the log said these
                // records were durable, and they are not all readable.
                if outcome.durable_verified < expected_durable {
                    outcome.violations.push(Violation::MissingDurableRecord {
                        sequence: next_sequence,
                        reason,
                    });
                }
                break;
            }
        }
    }

    // Running out of bytes counts too: a durable record that is simply absent
    // is as much a violation as one that is torn.
    if outcome.durable_verified < expected_durable
        && !outcome
            .violations
            .iter()
            .any(|v| matches!(v, Violation::MissingDurableRecord { .. }))
    {
        outcome.violations.push(Violation::MissingDurableRecord {
            sequence: next_sequence,
            reason: outcome.tail_stopped.unwrap_or(Torn::Truncated),
        });
    }

    outcome
}
