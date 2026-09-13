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
//! **The key set is checked, but not here.** This module reads a row it is
//! handed and has no way to know which keys were owed; the contract lives beside
//! the renderer that owes them, as `topology_report::MEASURED_ROW_KEYS`. An
//! earlier version of this paragraph said the check was deferred until the row
//! became a typed value, "at which point the key set is derivable from the type"
//! -- which is false, and was measured to be: a type says "a map of names to
//! values", which every key set satisfies, including the one missing a field.

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
    /// The row is not a syntactically valid JSON object.
    ///
    /// **Decided by a real parse, not by a check written here.** The question
    /// this answers is "could a consumer read this row", and a consumer uses a
    /// JSON parser -- so the only answer that cannot drift from the question is
    /// one a JSON parser gives. Two hand-written versions preceded this: the
    /// first counted bracket depth, which `{"a":1,}` and `{"a":1]` both satisfy;
    /// the second matched delimiters by kind and checked separators, and a
    /// generated test still found 159 rows it accepted and `serde_json` did not.
    Malformed {
        /// What is wrong, in the parser's own words.
        what: String,
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
            Self::Malformed { what, row } => {
                write!(f, "the row is not a valid JSON object -- {what}: {row}")
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

    if let Some(what) = malformation(row) {
        found.push(RowDefect::Malformed {
            what,
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

/// What is wrong with `row` as a JSON object, if anything.
///
/// **A real parse, because the question is whether a consumer can parse it.**
/// Anything else here is a second opinion about what JSON is, and a second
/// opinion is a thing that can disagree. Both hand-written predecessors did:
/// the first counted bracket depth and accepted `{"a":1,}`; the second matched
/// delimiters by kind and checked separators, and a test that generated 1807
/// single-character corruptions of a real row found **159 it accepted and
/// `serde_json` rejected -- every one a false accept.** Closing the last of them
/// required tracking whether an object expects a name or a value next, which is
/// a JSON parser; so this depends on one rather than growing one.
///
/// **A parse does not subsume [`RowDefect::RepeatedKey`].** `serde_json` accepts
/// a duplicated key and silently keeps the last, which is exactly why that
/// defect is worth a check of its own: it survives the consumer's parse and
/// changes what the consumer reads. The two checks answer different questions
/// and neither replaces the other.
fn malformation(row: &str) -> Option<String> {
    // The row is required to be an OBJECT, not merely valid JSON. A bare `[1,2]`
    // parses and would satisfy a laxer check, while carrying no keys at all.
    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(row) {
        Ok(_) => None,
        Err(error) => Some(error.to_string()),
    }
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
    let mut at = 0;

    while let Some(open) = next_string(row, at) {
        // Brackets BETWEEN strings are the only ones that count. Inside a
        // string they are text -- a failed discovery's message may contain any
        // of them.
        for character in row[at..open].chars() {
            match character {
                '[' | '{' => depth += 1,
                ']' | '}' => depth -= 1,
                _ => {}
            }
        }

        let Some(close) = string_end(row, open + 1) else {
            break;
        };
        let name = &row[open + 1..close];
        let tail = row[close + 1..].trim_start();

        // A name followed by `:` at depth 1 is a key of the row itself. Anything
        // else is a value, or a key of a nested object.
        if tail.starts_with(':') && depth == 1 {
            names.push(name);
        }

        at = close + 1;
    }

    names
}

/// Where the next string starts at or after `from`.
///
/// There is nothing to skip here -- a quote outside a string always opens one --
/// but it is named so the pair with [`string_end`] reads as a scan rather than
/// as two bare `find` calls.
fn next_string(row: &str, from: usize) -> Option<usize> {
    row[from..].find('"').map(|at| from + at)
}

/// Where the string opening before `from` closes, honouring `\` escapes.
///
/// **This is the half [`keys`] was missing, and it was reachable.** `keys` used
/// `find('"')`, which takes `\"` for a terminator -- so a `discovery_error`
/// carrying an escaped quote shifted the parser's idea of where strings begin
/// and end, and text INSIDE the error was emitted as top-level keys. Two equal
/// ones then read as a repeated key.
///
/// Measured before this fix: `report_unmeasured` given an `io::Error` of
/// `q":1,"q":1,"q` rendered a row `JSON.parse` accepts with four keys, and
/// `assert_corresponds` panicked from inside the renderer -- a correct report
/// crashing the probe, which is the failure mode containment exists to prevent.
/// [`balanced`] already had this state machine; `keys` did not, so the reader
/// disagreed with the writer.
fn string_end(row: &str, from: usize) -> Option<usize> {
    let mut escaped = false;
    for (at, character) in row[from..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => return Some(from + at),
            _ => {}
        }
    }

    None
}

/// Where the list opening at `start` closes, if it closes.
///
/// Depth-aware, because each entry is an object: the first `]` after the opening
/// bracket may belong to a nested list rather than to this one.
///
/// Public alongside [`list_codes`] because an instrument that SABOTAGES a list
/// needs the same span the reader uses. A test that cut on commas instead broke
/// silently when the entries became objects -- and a sabotage that no longer
/// sabotages leaves the rule it guards unguarded while still passing.
#[must_use]
pub fn list_span_end(row: &str, start: usize) -> Option<usize> {
    let mut depth = 0_i32;
    for (at, character) in row[start..].char_indices() {
        match character {
            '[' | '{' => depth += 1,
            '}' => depth -= 1,
            ']' if depth == 0 => return Some(start + at),
            ']' => depth -= 1,
            _ => {}
        }
    }

    None
}

/// The `code` of every entry in `row`'s list-valued `key`.
///
/// **One definition, because two instruments need it.** The diagnostic lists
/// hold objects -- `{"code":"contradictory_cores","count":3}` -- so reading them
/// means finding each entry's `code` member rather than splitting on commas,
/// which nested objects break. Both the unit tests and the publication
/// accounting ask this question, and a second implementation of it is the kind
/// of copy that agrees until it does not.
///
/// Returns empty for a key that is absent or not a list, which is the same
/// answer as an empty list on purpose: a consumer of this is asking "what
/// conditions are published", and "none" is the answer in both cases.
#[must_use]
pub fn list_codes(row: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\":[");
    let Some(start) = row.find(&needle).map(|at| at + needle.len()) else {
        return Vec::new();
    };
    let Some(end) = list_span_end(row, start) else {
        return Vec::new();
    };

    let mut codes = Vec::new();
    let mut rest = &row[start..end];
    const CODE: &str = "\"code\":\"";
    while let Some(at) = rest.find(CODE) {
        let after = &rest[at + CODE.len()..];
        let Some(close) = after.find('"') else {
            break;
        };
        codes.push(after[..close].to_owned());
        rest = &after[close..];
    }

    codes
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
    (rows.next().is_none() && malformation(row).is_none()).then_some(row)
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
