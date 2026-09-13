// Copyright (c) Mike Grier.

//! Whether a rendered report's machine-readable row is well-formed.
//!
//! # What this used to be, and why it is not that any more
//!
//! This module was an oracle over the report's PROSE: it read the rendered
//! sentences, extracted values back out of them, and compared those against the
//! NDJSON row. It was built after a pull-request review found a report calling a
//! state a bug while the verdict two paragraphs below certified the same run as
//! `agree`.
//!
//! [DESIGN-NOTES.md](../DESIGN-NOTES.md#d-encoded-row-is-the-contract) retired
//! that design. The row is a machine contract -- mined across a fleet, and what
//! this workspace's designs rest on -- and the prose is for a reader. They carry
//! different obligations: the row must be CORRECT, machine-enforced; the prose
//! must be ACCURATE AND READABLE, enforced by review. Nothing is required to
//! hold *between* them.
//!
//! The correspondences worth keeping were never about rendering. They related a
//! STATE to the verdict, and they live in [`crate::topology::invariant`] now, as
//! predicates over the observation that run whether or not anything was
//! rendered. Of the thirty-eight functions this module carried, twenty-three
//! existed only to extract values back out of rendered text -- a parser for a
//! format this crate itself writes, and it behaved like one: a multi-byte panic,
//! a substring matching inside an opaque `io::Error`, a `trim_matches`
//! collapsing `[[0]]` and `[0]`. None of those was a defect in a probe.
//!
//! # What is left, and why anything is left at all
//!
//! Structure makes most of the old checks unrepresentable rather than detected,
//! which is the stronger move. What structure cannot check is **the writer** --
//! whatever turns values into bytes is downstream of every type, and several of
//! this crate's defects lived exactly there. So one check survives: the report
//! carries exactly one machine-readable row, and that row is a well-formed flat
//! JSON object.
//!
//! That is not a correspondence. It is the writer's own output being read back,
//! which is the one thing no amount of typing upstream can do for itself.
//!
//! **The key SET is deliberately not checked here yet.** Asserting it needs a
//! list of expected keys, and a list written here would be a census that rots --
//! this component has re-corrected the same census three times in one day. Once
//! M3.3 makes the row a typed value, the key set is derivable from the type
//! rather than declared beside it, and the check belongs there. Queued in
//! [CHECKLIST.md](../CHECKLIST.md) M3.3 rather than approximated here.

/// A way the report's machine-readable row is not well-formed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowDefect {
    /// The report carries no machine-readable row.
    ///
    /// Every report has one, including the unmeasured shape -- that is what lets
    /// a fleet survey tell a host where discovery FAILED from a job that never
    /// ran the probe. A report without one silently excludes exactly the hosts
    /// most worth counting.
    Missing,
    /// The report carries more than one.
    ///
    /// A mining pass reads the first line that looks like a row, so a second one
    /// is not extra data -- it is an ambiguity about which line is the contract.
    /// Measured before containment existed: an `io::Error` whose text contained
    /// `{` was selected as the row, so a reader checked the caller's text
    /// instead of the probe's.
    Duplicated {
        /// How many lines look like a row.
        count: usize,
    },
    /// The row's brackets do not balance, so it is not a JSON object.
    Unbalanced {
        /// The row, as rendered.
        row: String,
    },
    /// The row repeats a key.
    ///
    /// A repeated key is not a parse error in every JSON reader -- most take the
    /// last -- so this is precisely the kind of malformation that survives a
    /// consumer's parse and changes what it reads.
    RepeatedKey {
        /// The key rendered more than once.
        key: String,
    },
}

impl std::fmt::Display for RowDefect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("the report carries no machine-readable row"),
            Self::Duplicated { count } => write!(
                f,
                "the report carries {count} machine-readable rows, so which one \
                 is the contract is ambiguous"
            ),
            Self::Unbalanced { row } => {
                write!(f, "the row's brackets do not balance: {row}")
            }
            Self::RepeatedKey { key } => write!(
                f,
                "the row renders `{key}` more than once, which most JSON readers \
                 resolve silently by taking the last"
            ),
        }
    }
}

/// Every way `report`'s machine-readable row is not well-formed.
#[must_use]
pub fn check(report: &str) -> Vec<RowDefect> {
    let rows: Vec<&str> = report
        .lines()
        .filter(|line| line.starts_with('{'))
        .collect();

    let [row] = rows.as_slice() else {
        return vec![if rows.is_empty() {
            RowDefect::Missing
        } else {
            RowDefect::Duplicated { count: rows.len() }
        }];
    };

    let mut found = Vec::new();

    if !balanced(row) {
        found.push(RowDefect::Unbalanced {
            row: (*row).to_owned(),
        });
        // Every check below reads the object's members, which is not a question
        // that means anything about text that is not an object.
        return found;
    }

    let mut seen: Vec<&str> = Vec::new();
    for key in keys(row) {
        if seen.contains(&key) {
            found.push(RowDefect::RepeatedKey {
                key: key.to_owned(),
            });
        } else {
            seen.push(key);
        }
    }

    found
}

/// Whether every bracket in `row` is closed, in order.
fn balanced(row: &str) -> bool {
    let mut depth = 0_i32;
    let mut in_string = false;
    let mut escaped = false;

    for character in row.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '[' | '{' if !in_string => depth += 1,
            ']' | '}' if !in_string => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }

    depth == 0 && !in_string
}

/// Every key `row` renders at its top level, in the order it renders them.
///
/// Top level only, deliberately: a nested object's members are that object's
/// keys, and repeating one there is a different question from repeating one in
/// the row.
#[must_use]
pub fn keys(row: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut depth = 0_i32;
    let mut rest = row;

    while let Some(quote) = rest.find('"') {
        for character in rest[..quote].chars() {
            match character {
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            }
        }

        let after = &rest[quote + 1..];
        let Some(end) = after.find('"') else {
            break;
        };
        let name = &after[..end];
        let tail = after[end + 1..].trim_start();

        // A name followed by `:` at depth 1 is a key of the row itself. Anything
        // else is a value, or a key of a nested object.
        if tail.starts_with(':') && depth == 1 {
            names.push(name);
        }

        rest = &after[end + 1..];
    }

    names
}

/// The report's machine-readable row, if it carries exactly one well-formed one.
///
/// Public because the instruments in `tests/` read the row to ask what it
/// publishes, and a second implementation of "which line is the row" is the kind
/// of copy that agrees until it does not.
#[must_use]
pub fn row(report: &str) -> Option<&str> {
    let mut rows = report.lines().filter(|line| line.starts_with('{'));
    let row = rows.next()?;
    (rows.next().is_none() && balanced(row)).then_some(row)
}

/// [`check`], as an assertion, for tests that render a report.
///
/// # Panics
///
/// Panics listing every way the row is malformed.
pub fn assert_corresponds(report: &str) {
    let defects = check(report);
    assert!(
        defects.is_empty(),
        "a rendered report's machine-readable row is malformed in {} way(s):\n{}\n\n\
         --- the report ---\n{report}",
        defects.len(),
        defects
            .iter()
            .map(|defect| format!("  - {defect}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[cfg(test)]
mod tests;
