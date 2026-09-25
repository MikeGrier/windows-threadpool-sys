// Copyright (c) Mike Grier
//! Every clause in the response space is checked by something (M26.6).
//!
//! # What this is for
//!
//! `M26` split one job in two. The resolver sweeps the *permissions* -- the
//! `RS-P-n` clauses, which say what a platform may do -- and the kernel tests
//! confirm that a real Windows stays inside the *constraints*, the `RS-C-n`
//! clauses, which say what this crate requires of it. That division is stated
//! in [RESPONSE-SPACE.md](../RESPONSE-SPACE.md), and `RS-C-4` in particular is
//! justified there **on the strength of these tests existing**: the resolver is
//! forbidden to break the drain half of `DRAIN_PRECEDING_OPS`, so a Windows
//! that broke it would be caught by nothing the resolver does.
//!
//! A division of labour recorded only in prose is enforced by whoever
//! remembers it, which over a long change is nobody. This file is the rung
//! below prose: it reads the space, finds every clause, and fails when one is
//! cited by nothing on the side that owes it a check.
//!
//! # Why a marker, and not a search for the clause ID
//!
//! The first version of this census searched each file for the clause ID
//! anywhere in its text, and it was **measured green while broken**. Removing
//! `RS-C-4`'s check from the only test that performs it did not turn it red,
//! because a second file mentioned that clause only to say the check was
//! somebody else's -- and a disclaimer reads identically to a claim under a
//! substring search. That is the same trap this repository already recorded
//! once, where a bare substring matched a probe whose only mention of a tag
//! was a comment.
//!
//! So a claim is now a **structured marker** -- `CONFIRMS:` for a constraint
//! checked against a real kernel, `EXERCISES:` for a permission the resolver
//! takes -- and prose mentioning a clause means nothing. The markers are
//! verified to go red in both directions before being trusted.
//!
//! # Why a census over source is sound here, when it usually is not
//!
//! This repository has been burned by proxies: a test that walked `src/bin`
//! and grepped for a substring was replaced because emitting a row is a
//! property of a program's *output*, which no read of its source can
//! establish. The distinction is that **the claim here is itself about the
//! source**. "Does a kernel test claim this clause" is a fact about what is
//! written in `tests/`, so reading `tests/` is the direct measurement rather
//! than a stand-in for one.
//!
//! What that buys is narrow, and saying so is the point: this proves a clause
//! is *claimed* by a file, never that the file's assertions are adequate. That
//! second question is what calibration is for -- `M26.5` re-injected two real
//! defects to show the instruments go red -- and no census can answer it.

#![cfg(windows)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Marker for "this file confirms a real kernel stays inside this clause".
const CONFIRMS: &str = "CONFIRMS: ";
/// Marker for "this file exercises this freedom of the resolver".
const EXERCISES: &str = "EXERCISES: ";

/// The crate root, so this reads the same files whatever the working
/// directory is -- including the scratch copy `cargo-mutants` and the sabotage
/// harness build from.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Clause IDs declared in the space, read from their headings.
///
/// Derived from the document rather than listed here, so a clause added to the
/// space is immediately owed a check rather than silently exempt. That is the
/// same rule the resolver follows against the same document.
fn clauses_in_the_space() -> BTreeSet<String> {
    let path = crate_root().join("RESPONSE-SPACE.md");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    text.lines()
        .filter_map(|line| {
            let heading = line.strip_prefix("### ")?;
            let id = heading.split_whitespace().next()?;
            (id.starts_with("RS-P-") || id.starts_with("RS-C-")).then(|| id.to_owned())
        })
        .collect()
}

/// Every clause claimed by `marker` in `text`.
///
/// A marker must be the whole of what follows it on its line, so a sentence
/// that happens to contain the word cannot become a claim by accident.
fn claims(text: &str, marker: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let at = line.find(marker)?;
            let id = line[at + marker.len()..].trim();
            (!id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
                .then(|| id.to_owned())
        })
        .collect()
}

/// Every `.rs` file directly under `tests/`, with its text.
fn test_files() -> BTreeMap<String, String> {
    let dir = crate_root().join("tests");
    std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("reading {}: {error}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a UTF-8 file name")
                .to_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
            (name, text)
        })
        .collect()
}

/// The resolver's own unit tests, which live in `src/` and carry the
/// `EXERCISES:` markers.
fn resolver_unit_tests() -> String {
    let path = crate_root()
        .join("src")
        .join("sys")
        .join("resolver")
        .join("tests.rs");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// This file, whose own text names the markers and must never be counted as
/// claiming anything.
const SELF: &str = "response_space_census.rs";

#[test]
fn every_constraint_is_confirmed_against_a_real_kernel() {
    // RS-C-n says what this crate requires of the platform. The resolver is
    // forbidden to violate these, so the resolver can never be the thing that
    // checks them -- only a test against a real ring can.
    let files = test_files();
    let constraints: Vec<String> = clauses_in_the_space()
        .into_iter()
        .filter(|id| id.starts_with("RS-C-"))
        .collect();

    // Vacuity guard, and not a formality: a parse that silently found nothing
    // would make every assertion below pass while checking no clause at all.
    assert!(
        constraints.len() >= 4,
        "only {} constraints parsed out of RESPONSE-SPACE.md, which means the heading format \
         changed and this census is reading nothing",
        constraints.len()
    );

    let confirmed: BTreeMap<String, Vec<String>> = files
        .iter()
        .filter(|(name, _)| name.as_str() != SELF)
        .fold(BTreeMap::new(), |mut acc, (name, text)| {
            for clause in claims(text, CONFIRMS) {
                acc.entry(clause).or_default().push(name.clone());
            }
            acc
        });

    let unchecked: Vec<&String> = constraints
        .iter()
        .filter(|clause| !confirmed.contains_key(*clause))
        .collect();

    assert!(
        unchecked.is_empty(),
        "these constraints carry no CONFIRMS marker in any kernel test, so nothing confirms \
         Windows stays inside them: {unchecked:?}\n\
         RESPONSE-SPACE.md justifies constraining the resolver away from these on the grounds \
         that the kernel tests cover them. A constraint checked on neither side is untested in \
         both halves at once, which is the hole M26.6 exists to close.\n\
         Claimed today: {confirmed:?}"
    );
}

#[test]
fn every_permission_is_exercised_by_a_resolver_test() {
    // The other direction, and the reason to have both: a permission the
    // resolver never takes is indistinguishable from one it never implemented,
    // and a suite written against it would be hardened for nothing.
    let permissions: Vec<String> = clauses_in_the_space()
        .into_iter()
        .filter(|id| id.starts_with("RS-P-"))
        .collect();
    assert!(
        permissions.len() >= 7,
        "only {} permissions parsed out of RESPONSE-SPACE.md; the heading format changed",
        permissions.len()
    );

    let mut exercised = claims(&resolver_unit_tests(), EXERCISES);
    for (name, text) in test_files() {
        if name.as_str() != SELF {
            exercised.extend(claims(&text, EXERCISES));
        }
    }

    let unexercised: Vec<&String> = permissions
        .iter()
        .filter(|clause| !exercised.contains(*clause))
        .collect();

    assert!(
        unexercised.is_empty(),
        "these permissions carry no EXERCISES marker: {unexercised:?}\n\
         A clause the resolver never exercises is permitted on paper and implemented nowhere, \
         which is the failure RESPONSE-SPACE.md's clause IDs exist to make visible.\n\
         Claimed today: {exercised:?}"
    );
}

#[test]
fn a_marker_is_a_claim_and_a_mention_is_not() {
    // The distinction this census was rebuilt around, asserted rather than
    // trusted -- because the version that conflated the two was green while
    // failing to detect a removed check.
    let disclaimer = "//! The drain clause RS-C-4 is somebody else's job entirely.";
    assert!(
        claims(disclaimer, CONFIRMS).is_empty(),
        "prose naming a clause must not count as claiming it"
    );

    let claim = "//! CONFIRMS: RS-C-4";
    assert_eq!(
        claims(claim, CONFIRMS),
        BTreeSet::from(["RS-C-4".to_owned()]),
        "a marker line must be read as a claim"
    );

    // A marker with trailing prose is not a claim either: allowing it would
    // let "CONFIRMS: RS-C-4 (eventually, once someone writes it)" pass.
    let hedged = "//! CONFIRMS: RS-C-4 eventually";
    assert!(
        claims(hedged, CONFIRMS).is_empty(),
        "a marker must name a clause and nothing else"
    );
}

#[test]
fn every_marker_names_a_clause_the_space_declares() {
    // A clause renamed in the document and not in the markers would leave the
    // two censuses above checking a clause that no longer exists, and passing.
    let declared = clauses_in_the_space();
    let mut sources = test_files();
    sources.insert("resolver/tests.rs".to_owned(), resolver_unit_tests());

    let mut dangling: BTreeSet<String> = BTreeSet::new();
    for (name, text) in &sources {
        if name.as_str() == SELF {
            continue;
        }
        for marker in [CONFIRMS, EXERCISES] {
            for id in claims(text, marker) {
                if !declared.contains(&id) {
                    dangling.insert(format!("{name}: {marker}{id}"));
                }
            }
        }
    }

    assert!(
        dangling.is_empty(),
        "these markers name clauses RESPONSE-SPACE.md does not declare: {dangling:?}"
    );
}
