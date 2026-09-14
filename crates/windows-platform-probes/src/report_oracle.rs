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
//! carries exactly one machine-readable row, and that row is a well-formed JSON
//! object. **Not flat** -- an earlier version of this sentence said flat, which
//! the row has not been since it began publishing diagnostics: `caches`,
//! `policies` and the three diagnostic lists are nested arrays and objects. A
//! reader who believed it would have taken the nested data for a defect.
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

    let mut seen: Vec<String> = Vec::new();
    for key in keys(row) {
        if seen.contains(&key) {
            found.push(RowDefect::RepeatedKey { key });
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

/// Every key `row` renders at its top level, in the order it renders them, and
/// INCLUDING repeats.
///
/// **Read from the parser's own tokens, not by walking the bytes.** Both
/// properties this returns are ones a parsed map destroys: `serde_json::Map`
/// sorts its names, and silently keeps the last of a repeated key -- which is
/// exactly the defect [`RowDefect::RepeatedKey`] reports, so parsing into a map
/// would delete the evidence. A `MapAccess` visitor sees each name as the parser
/// reads it, which keeps both while leaving every escape, quote and delimiter
/// decision to `serde_json`.
///
/// The hand-written version of this was the last string scanner here, and it had
/// already produced a real defect: it used `find('"')`, which takes `\"` for a
/// terminator, so a `discovery_error` carrying an escaped quote shifted where it
/// thought strings began and text INSIDE the error was emitted as top-level
/// keys. Measured: an `io::Error` of `q":1,"q":1,"q` rendered a row that
/// `JSON.parse` accepts with four keys, and `assert_row_is_well_formed` panicked from
/// inside the renderer. Fixing that added escape-awareness to one of the
/// scanners and left the others to be argued about; this removes the question.
///
/// Top level only, deliberately: a nested object's members are that object's
/// keys, and repeating one there is a different question from repeating one in
/// the row. The visitor reads nested values as [`serde::de::IgnoredAny`], which
/// consumes them without collecting their names.
#[must_use]
pub fn keys(row: &str) -> Vec<String> {
    struct TopLevelNames;

    impl<'de> serde::de::Visitor<'de> for TopLevelNames {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the probe's machine-readable row, a JSON object")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut members: A,
        ) -> Result<Self::Value, A::Error> {
            let mut names = Vec::new();
            while let Some(name) = members.next_key::<String>()? {
                names.push(name);
                members.next_value::<serde::de::IgnoredAny>()?;
            }
            Ok(names)
        }
    }

    let mut reader = serde_json::Deserializer::from_str(row);
    serde::Deserializer::deserialize_map(&mut reader, TopLevelNames).unwrap_or_default()
}
/// The `code` of every entry in `row`'s list-valued `key`.
///
/// **One definition, because two instruments need it.** The diagnostic lists
/// hold objects -- `{"code":"contradictory_cores","count":3}` -- and both the
/// unit tests and the publication accounting ask this question. A second
/// implementation of it is the kind of copy that agrees until it does not.
///
/// Read from a parse. The previous version searched for `"code":"` and then took
/// the next `"` as the end, which is not escape-aware: a code containing a quote
/// would have truncated. That was argued safe because every code is a
/// `&'static str` from an enum and no caller text reaches a list -- an argument
/// that was true, load-bearing, and enforced by nothing. Parsing makes the
/// argument unnecessary rather than merely correct, which is the difference
/// between a property and a hope.
///
/// Returns empty for a key that is absent or not a list, which is the same
/// answer as an empty list on purpose: a consumer of this is asking "what
/// conditions are published", and "none" is the answer in both cases.
#[must_use]
pub fn list_codes(row: &str, key: &str) -> Vec<String> {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(row) else {
        return Vec::new();
    };
    let Some(entries) = parsed.get(key).and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| entry.get("code"))
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}
/// The report's machine-readable row, if it carries exactly one well-formed one.
///
/// Public because the instruments in `tests/` read the row to ask what it
/// publishes, and a second implementation of "which line is the row" is the kind
/// of copy that agrees until it does not.
///
/// **Defined by [`check`], so the accessor and the oracle cannot disagree.** It
/// ran its own subset -- one row, and `malformation` -- and `serde_json` accepts
/// a duplicated key, so a row that `check` reported as
/// [`RowDefect::RepeatedKey`] was handed back here as well-formed. A caller
/// asking "may I read this row" got yes for a row the crate had already judged
/// ambiguous, which is the one malformation that survives a consumer's parse and
/// changes what it reads. Found by a review.
///
/// This is the same defect the module keeps warning about, in the function whose
/// doc comment warns about it: two implementations of one question, agreeing
/// until they did not.
#[must_use]
pub fn row(report: &str) -> Option<&str> {
    if !check(report).is_empty() {
        return None;
    }
    report.lines().find(|line| line.starts_with('{'))
}

use crate::row::Shape;

/// Every way `row` departs from `schema`'s value SHAPES, as sentences.
///
/// **The half the key contract was missing.** `MEASURED_ROW_KEYS` pins which
/// names appear and in what order, and says nothing about what they hold -- so
/// a renderer could publish `"processors":"16"` and satisfy the key test, the
/// well-formedness check and every renderer assertion at once. Measured before
/// this existed: rendering that one field through `.to_string()` left all 230
/// library tests and all 10 real-host integration tests green. Reported by a
/// review.
///
/// Keys are not re-checked here; that is the key test's job, and doing it in
/// both places would make one of them the copy. A key the schema names and the
/// row lacks is reported, because a shape cannot be checked against nothing.
#[must_use]
pub fn shape_violations(row: &str, schema: &[(&str, Shape)]) -> Vec<String> {
    use serde_json::Value as Json;

    let Ok(parsed) = serde_json::from_str::<serde_json::Map<String, Json>>(row) else {
        return vec![format!("the row is not a JSON object: {row}")];
    };

    let is_number = |value: &Json| value.is_u64() || value.is_i64();
    let coded = |value: &Json| {
        value
            .as_object()
            .is_some_and(|entry| entry.get("code").is_some_and(Json::is_string))
    };

    let mut found = Vec::new();
    for (name, shape) in schema {
        let Some(value) = parsed.get(*name) else {
            found.push(format!("`{name}` is missing, so its shape cannot hold"));
            continue;
        };

        let holds = match shape {
            Shape::Text => value.is_string(),
            Shape::Number => is_number(value),
            Shape::NumberOrNull => is_number(value) || value.is_null(),
            Shape::ListOfNumbers => value.as_array().is_some_and(|l| l.iter().all(is_number)),
            Shape::ListOfObjects => value
                .as_array()
                .is_some_and(|l| l.iter().all(Json::is_object)),
            Shape::ListOfCoded => value.as_array().is_some_and(|l| l.iter().all(coded)),
            Shape::ObjectOfNumbers => value
                .as_object()
                .is_some_and(|o| o.values().all(&is_number)),
        };

        if !holds {
            found.push(format!("`{name}` should be {shape:?} but is `{value}`"));
        }
    }

    found
}

/// [`shape_violations`], as an assertion.
///
/// # Panics
///
/// Panics listing every value whose shape the schema forbids.
pub fn assert_row_has_the_schemas_shapes(report: &str, schema: &[(&str, Shape)]) {
    let row = row(report).unwrap_or_else(|| panic!("no single well-formed row in:\n{report}"));
    let violations = shape_violations(row, schema);
    assert!(
        violations.is_empty(),
        "the row departs from its schema in {} way(s):\n{}\n\n--- the row ---\n{row}",
        violations.len(),
        violations
            .iter()
            .map(|what| format!("  - {what}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

/// [`check`], as an assertion, for tests that render a report.
///
/// **Named `assert_corresponds` until 2026-09-13, and the name outlived what it
/// did.** Before M3 this compared a report's prose against its encoded row; M3
/// made the row the machine contract and retired that comparison, leaving a
/// function that validates ONE thing -- that the report carries exactly one
/// well-formed JSON row. The old name went on promising a cross-part guarantee
/// to every reader of its four call sites. Renamed after a review read those
/// call sites as still enforcing correspondence, which is exactly the mistake
/// the name invited.
///
/// # Panics
///
/// Panics listing every way the row is malformed.
pub fn assert_row_is_well_formed(report: &str) {
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
