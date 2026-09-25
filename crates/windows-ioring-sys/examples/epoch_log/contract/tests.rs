// Copyright (c) 2026 Mike Grier
//! Tests for the contract's *presentable* form (M23.1).
//!
//! # What these check, and what they deliberately do not
//!
//! They check properties of what the program **prints**: that every clause the
//! report iterates has something to say, that two clauses cannot arrive under
//! one heading, and that no statement renders as an empty or ragged bullet.
//! Those are facts about the output, and each is a way the report could
//! silently degrade while every other test in the sample stayed green.
//!
//! They deliberately do **not** assert that any particular rule is present --
//! no test here looks for the word "ring", or counts the assumptions. The
//! repository's CONTRACT INTEGRITY rule is explicit that a hand-written second
//! copy of a contract rule is not a check of the contract, it is a check of the
//! copy: such a test passes exactly when the two copies agree, says nothing
//! about whether either is right, and adds a second site to edit whenever the
//! contract changes. The prose in the module documentation is the authoritative
//! form and [`CONTRACT`] is its presentable reduction; what is worth testing is
//! that the reduction survives being printed.
//!
//! The one guard that genuinely belongs at the build rung is already there:
//! [`Clause::heading`]'s `match` is exhaustive, so a new variant cannot be
//! added without the compiler stopping at the site that must handle it.

use super::{CONTRACT, Clause};

/// Every clause the report walks has at least one statement.
///
/// The failure this prevents is quiet: [`Clause::ALL`] drives the report's
/// section headings, so a clause with no statements prints its heading and then
/// nothing, which reads as "this log promises nothing" rather than as a missing
/// entry.
#[test]
fn every_clause_has_at_least_one_statement() {
    for clause in Clause::ALL {
        let count = CONTRACT.iter().filter(|s| s.clause == clause).count();
        assert!(
            count > 0,
            "clause {clause:?} (\"{}\") has no statements, so the report would print an empty \
             section under its heading",
            clause.heading()
        );
    }
}

/// Every statement belongs to a clause the report actually walks.
///
/// The mirror of the test above, and the direction that is easy to forget: the
/// first proves `ALL` is covered by `CONTRACT`, this proves `CONTRACT` is
/// covered by `ALL`. A statement whose clause is missing from `ALL` is never
/// printed at all -- written down, compiled, and silently absent from the
/// report a reader is told to trust.
#[test]
fn every_statement_is_reachable_from_the_report() {
    for statement in CONTRACT {
        assert!(
            Clause::ALL.contains(&statement.clause),
            "statement {:?} has clause {:?}, which Clause::ALL does not list, so the report \
             would never print it",
            statement.text,
            statement.clause
        );
    }
}

/// No two clauses share a heading, and none is blank.
///
/// Two clauses printing under one heading would merge distinct parts of the
/// contract in the output -- a caller reading "this log guarantees" would be
/// shown things it explicitly does not guarantee, which is the worst available
/// failure for this particular program.
#[test]
fn headings_are_distinct_and_non_empty() {
    for (index, clause) in Clause::ALL.iter().enumerate() {
        let heading = clause.heading();
        assert!(!heading.trim().is_empty(), "{clause:?} has a blank heading");
        for other in &Clause::ALL[index + 1..] {
            assert_ne!(
                heading,
                other.heading(),
                "{clause:?} and {other:?} would print under the same heading"
            );
        }
    }
}

/// `Clause::ALL` lists each clause once.
///
/// A repeat would print a whole section twice. Worth a test rather than a
/// glance because `ALL` is hand-maintained -- the exhaustive `match` in
/// `heading` forces a new variant to be *handled*, but nothing forces it to be
/// added to `ALL` exactly once.
#[test]
fn all_lists_each_clause_once() {
    for (index, clause) in Clause::ALL.iter().enumerate() {
        assert!(
            !Clause::ALL[index + 1..].contains(clause),
            "{clause:?} appears more than once in Clause::ALL"
        );
    }
}

/// Statements render as clean bullets.
///
/// The report prints each as `  - {text}`, so leading or trailing whitespace
/// shows up as a ragged list and an empty statement as a bare dash. Both are
/// the kind of thing a line-continuation edit introduces without anyone
/// noticing, since the source is wrapped across several lines.
#[test]
fn statement_text_is_clean() {
    for statement in CONTRACT {
        let text = statement.text;
        assert!(
            !text.is_empty(),
            "a {:?} statement is empty and would print as a bare dash",
            statement.clause
        );
        assert_eq!(
            text.trim(),
            text,
            "statement {text:?} has leading or trailing whitespace and would print ragged"
        );
        assert!(
            !text.contains('\n'),
            "statement {text:?} contains a newline, which would break the one-bullet-per-line \
             shape the report assumes"
        );
    }
}
