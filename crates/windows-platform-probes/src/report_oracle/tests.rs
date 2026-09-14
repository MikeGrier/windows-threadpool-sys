// Copyright (c) Mike Grier.

//! Tests for the row's well-formedness check.
//!
//! Half of these assert ACCEPTANCE. A check that fires on a legal row costs a
//! reader more than one that misses an illegal one, because noise trains them to
//! ignore the instrument -- and the reports this runs against are the ones a
//! fleet survey mines, so a false alarm is a false finding about a host.

use super::{RowDefect, check, keys, malformation, row};

/// A well-formed row, in the shape the renderer emits.
fn clean_row() -> String {
    concat!(
        r#"{"reason":"x-probe-topology","arch":"x86_64","processors":16,"#,
        r#""efficiency_classes":[0],"caches":[{"level":1,"domains":8}],"#,
        r#""policies":{"single":1},"cross_check":"agree","disagreements":[],"#,
        r#""parse_incomplete":[]}"#
    )
    .to_owned()
}

/// A report carrying `row` under a line of prose.
fn report_with(row: &str) -> String {
    format!("host:  x86_64 16p/8c\nsome prose the reader gets\n{row}")
}

#[test]
fn a_well_formed_row_is_accepted() {
    assert_eq!(check(&report_with(&clean_row())), Vec::new());
}

#[test]
fn a_report_with_no_row_is_a_defect() {
    // Every report has one, including the unmeasured shape -- that is what lets
    // a survey tell a host where discovery failed from a job that never ran the
    // probe.
    assert_eq!(
        check("host:  x86_64 16p/8c\nprose only, no row"),
        vec![RowDefect::Missing]
    );
}

#[test]
fn a_report_with_two_rows_is_a_defect() {
    // A mining pass reads the first line that looks like a row, so a second is
    // not extra data -- it is an ambiguity about which line is the contract.
    let two = format!("{}\n{}", report_with(&clean_row()), clean_row());

    assert_eq!(check(&two), vec![RowDefect::Duplicated { count: 2 }]);
}

/// What the parser says about `row`, for a test asserting a defect's identity
/// rather than its wording.
///
/// **The wording is `serde_json`'s, so this crate does not get to assert it.**
/// These three tests used to name the message, which was right while the check
/// was ours -- the message WAS the finding, and a test naming it pinned which
/// branch fired. It is now a dependency's string, and pinning it would assert a
/// thing we neither own nor promise: a wording change in a patch release would
/// redden tests about unclosed delimiters, which is a false finding about this
/// crate. What survives is what these tests are actually for -- that this input
/// is rejected, and that the whole row is carried back for a reader.
fn malformation_of(row: &str) -> String {
    serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(row)
        .expect_err("the fixture is meant to be malformed")
        .to_string()
}

#[test]
fn an_unclosed_delimiter_is_a_defect() {
    let truncated = r#"{"reason":"x-probe-topology","caches":[{"level":1}"#;

    assert_eq!(
        check(&report_with(truncated)),
        vec![RowDefect::Malformed {
            what: malformation_of(truncated),
            row: truncated.to_owned()
        }]
    );
}

#[test]
fn a_trailing_separator_is_a_defect() {
    // **Balanced but invalid**, which the depth-only check accepted. A writer
    // that emitted a separator for a member it then skipped produces exactly
    // this, and every bracket still matches.
    let trailing = r#"{"reason":"x-probe-topology","arch":"x86_64",}"#;

    assert_eq!(
        check(&report_with(trailing)),
        vec![RowDefect::Malformed {
            what: malformation_of(trailing),
            row: trailing.to_owned()
        }]
    );
}

#[test]
fn valid_json_that_is_not_an_object_is_a_malformation() {
    // **Pins the `Map` in `malformation`, which the corpus cannot reach.** The
    // generated corruptions are one-character mutations of a row, and none can
    // turn an object into a valid NON-object -- so a regression from
    // `serde_json::Map` to `serde_json::Value` would have left every test green.
    // Reported by a review.
    //
    // Called directly rather than through `check`, because `check` selects rows
    // by a leading `{` and these never get that far: through the public path a
    // bare list is `RowDefect::Missing`, not a malformed row. That makes the
    // requirement defence in depth rather than a reachable case -- said plainly
    // here, because the comment beside it reads as though `[1,2]` arrives, and
    // the honest claim is that the type is what stops it ever mattering.
    for not_an_object in [r#"[1,2]"#, "null", "3", r#""a string""#, "true"] {
        assert!(
            malformation(not_an_object).is_some(),
            "{not_an_object} is valid JSON but carries no keys, so it is not a row"
        );
    }

    // The control: the same call accepts an object, so the assertions above are
    // not passing merely because `malformation` rejects everything.
    assert_eq!(malformation(&clean_row()), None);
}

#[test]
fn a_mismatched_closing_delimiter_is_a_defect() {
    // Also balanced by depth, also invalid: an object closed by a bracket.
    let mismatched = r#"{"reason":"x-probe-topology","arch":"x86_64"]"#;

    assert_eq!(
        check(&report_with(mismatched)),
        vec![RowDefect::Malformed {
            what: malformation_of(mismatched),
            row: mismatched.to_owned()
        }]
    );
}

#[test]
fn a_nested_list_closed_as_an_object_is_a_defect() {
    // The inner case, so the stack is shown to be a stack rather than a pair of
    // counters that happen to agree at the end.
    let mismatched = r#"{"reason":"x","caches":[{"level":1}}}"#;

    assert!(
        matches!(
            check(&report_with(mismatched)).as_slice(),
            [RowDefect::Malformed { .. }]
        ),
        "{:?}",
        check(&report_with(mismatched))
    );
}

#[test]
fn a_brace_inside_a_string_does_not_confuse_the_delimiter_stack() {
    // The acceptance half of the stack: `discovery_error` carries an OS message,
    // which may contain any delimiter. Mis-stacking those would report every
    // such host as malformed.
    let row = r#"{"reason":"x","discovery_error":"failed at {[ and never closed"}"#;

    assert_eq!(check(&report_with(row)), Vec::new());
}

#[test]
fn a_bracket_inside_a_string_does_not_unbalance_a_row() {
    // **The acceptance half that matters most**, because the renderer does emit
    // brackets inside strings: a failed discovery's `io::Error` is interpolated
    // into a string value, and an OS message is free to contain one. A balance
    // check that counted them would report every such host as malformed.
    let with_brackets = r#"{"reason":"x-probe-topology","error":"failed at [0] {oops}"}"#;

    assert_eq!(check(&report_with(with_brackets)), Vec::new());
}

#[test]
fn an_escaped_quote_does_not_end_a_string() {
    // The other half of the same hazard: a `\"` inside an error message would
    // otherwise close the string early and put the rest of the message into
    // bracket-counting.
    let escaped = r#"{"reason":"x-probe-topology","error":"he said \"[\" and left"}"#;

    assert_eq!(check(&report_with(escaped)), Vec::new());
}

#[test]
fn an_escaped_quote_inside_a_value_does_not_forge_a_key() {
    // **A correct report crashed the probe, and this is the shape that did it.**
    // `keys` used `find('"')`, which takes `\"` for a terminator, so an escaped
    // quote shifted where it thought strings began and ended and text INSIDE a
    // value was emitted as a top-level key. Two equal ones read as a repeated
    // key, and `assert_row_is_well_formed` panicked from inside `report_unmeasured`.
    //
    // Reachable, not hypothetical: `discovery_error` carries a failed
    // discovery's `io::Error`, whose message is whatever the OS said.
    let forged = concat!(
        r#"{"reason":"x-probe-topology","arch":"x86_64","#,
        r#""discovery_error":"q\":1,\"q\":1,\"q"}"#
    );

    assert_eq!(
        keys(forged),
        vec!["reason", "arch", "discovery_error"],
        "the error's contents are a VALUE, however many quotes it contains"
    );
    assert_eq!(check(&report_with(forged)), Vec::new());
}

#[test]
fn a_backslash_before_the_closing_quote_does_not_swallow_the_rest_of_the_row() {
    // The other half: a value ending in an escaped backslash closes normally,
    // so the keys after it are still found. Getting this wrong in the other
    // direction would silently drop every key that follows.
    let row = r#"{"reason":"x","path":"C:\\temp\\","cross_check":"agree"}"#;

    assert_eq!(keys(row), vec!["reason", "path", "cross_check"]);
    assert_eq!(check(&report_with(row)), Vec::new());
}

#[test]
fn a_repeated_key_is_a_defect() {
    // Not a parse error in most readers -- they take the last -- so this is
    // precisely the malformation that survives a consumer's parse and changes
    // what it reads.
    let repeated = r#"{"reason":"x-probe-topology","processors":16,"processors":8}"#;

    assert_eq!(
        check(&report_with(repeated)),
        vec![RowDefect::RepeatedKey {
            key: "processors".to_owned()
        }]
    );
}

#[test]
fn a_key_repeated_inside_a_nested_object_is_not_the_rows_key() {
    // Top level only: a nested object's members are that object's keys, and
    // repeating one there is a different question. `policies` renders arbitrary
    // policy names, so a name colliding with a row key is possible.
    let nested = r#"{"reason":"x-probe-topology","processors":16,"policies":{"processors":2}}"#;

    assert_eq!(check(&report_with(nested)), Vec::new());
}

#[test]
fn the_rows_keys_are_read_at_the_top_level_only() {
    let row = clean_row();

    assert_eq!(
        keys(&row),
        vec![
            "reason",
            "arch",
            "processors",
            "efficiency_classes",
            "caches",
            "policies",
            "cross_check",
            "disagreements",
            "parse_incomplete",
        ],
        "the nested `level`, `domains` and `single` are not the ROW's keys"
    );
}

#[test]
fn the_row_accessor_declines_every_report_the_oracle_faults() {
    // **The accessor and the oracle answer one question, so they may not
    // disagree.** `row` ran its own subset -- one row, and `malformation` -- and
    // `serde_json` accepts a duplicated key, so a report `check` faulted as
    // `RepeatedKey` was handed back here as readable. A caller asking "may I
    // read this row" got yes for a row already judged ambiguous.
    //
    // Stated as the correspondence rather than as the one case, because the
    // gap was not in the case anyone wrote down: it was in the SECOND
    // implementation existing at all. Any future defect `check` learns is
    // covered here without a new test.
    let faulted = [
        report_with(r#"{"reason":"x","processors":4,"processors":8}"#),
        report_with(r#"{"reason":"x","arch":"x86_64",}"#),
        report_with(r#"{"reason":"x","arch":"x86_64"]"#),
        report_with(r#"{"unclosed":["#),
        "a report with no row at all".to_owned(),
        format!(
            "{}\n{}",
            r#"{"reason":"x","arch":"x86_64"}"#, r#"{"reason":"y","arch":"x86"}"#
        ),
    ];

    for report in &faulted {
        let defects = check(report);
        assert!(
            !defects.is_empty(),
            "the fixture must be faulted for this to mean anything: {report}"
        );
        assert_eq!(
            row(report),
            None,
            "`check` reports {defects:?} and `row` handed the row back anyway"
        );
    }

    // And the other direction, or the rule is satisfied by refusing everything.
    let clean = report_with(&clean_row());
    assert!(check(&clean).is_empty());
    assert!(
        row(&clean).is_some(),
        "a clean report must still be readable"
    );
}

#[test]
fn the_row_accessor_declines_an_ambiguous_or_malformed_report() {
    let one = report_with(&clean_row());
    let two = format!("{}\n{}", report_with(&clean_row()), clean_row());
    let malformed = report_with(r#"{"unbalanced":["#);

    assert_eq!(row(&one), Some(clean_row().as_str()));
    assert_eq!(row("prose only"), None);
    assert_eq!(row(&two), None);
    assert_eq!(row(&malformed), None);
}

#[test]
fn a_defect_is_reported_once_per_extra_rendering_of_a_key() {
    // **One entry per EXTRA rendering, so the count reads as how many times the
    // row said it again.** Three renderings of `a` give two defects, not one and
    // not three.
    //
    // This was named `..._once_per_repeated_key_rather_than_per_occurrence` and
    // opened by claiming de-duplication the code does not do -- while asserting
    // the per-occurrence behaviour its own failure message describes. A reader
    // taking the name for the contract got it backwards. The assertion was
    // right; the name and the comment were the defect.
    let thrice = r#"{"a":1,"a":2,"a":3,"b":1,"b":2}"#;

    assert_eq!(
        check(&report_with(thrice)),
        vec![
            RowDefect::RepeatedKey {
                key: "a".to_owned()
            },
            RowDefect::RepeatedKey {
                key: "a".to_owned()
            },
            RowDefect::RepeatedKey {
                key: "b".to_owned()
            },
        ],
        "one entry per EXTRA rendering, so the count reads as how many times the \
         row said it again"
    );
}

/// Every single-character corruption of `row`, as (what was done, the result).
///
/// Deletion and structural substitution, which between them reach the defects a
/// writer actually produces: a lost delimiter, a doubled separator, a `:` where
/// a `,` belonged, a quote that ends a string early.
fn corruptions(row: &str) -> Vec<(String, String)> {
    const STRUCTURAL: [char; 8] = ['{', '}', '[', ']', ',', ':', '"', '\\'];
    let mut out = Vec::new();

    for (at, character) in row.char_indices() {
        let after = at + character.len_utf8();

        let mut deleted = String::with_capacity(row.len());
        deleted.push_str(&row[..at]);
        deleted.push_str(&row[after..]);
        out.push((format!("deleted {character:?} at {at}"), deleted));

        for replacement in STRUCTURAL {
            if replacement == character {
                continue;
            }
            let mut swapped = String::with_capacity(row.len() + 1);
            swapped.push_str(&row[..at]);
            swapped.push(replacement);
            swapped.push_str(&row[after..]);
            out.push((
                format!("replaced {character:?} at {at} with {replacement:?}"),
                swapped,
            ));
        }
    }

    out
}

#[test]
fn every_unparseable_row_reaches_the_caller_as_a_defect() {
    // **This is a WIRING test, and saying so matters.** An earlier version of it
    // compared `malformation` against `serde_json` and was worth running,
    // because `malformation` was then a hand-written check that could disagree
    // -- it generated 1807 corruptions and found 159 disagreements, every one a
    // false accept, which is why the hand-written version is gone.
    //
    // With the parse itself delegated, that comparison would be `serde_json`
    // against `serde_json`: green by construction, and exactly the kind of
    // tautology this crate keeps having to delete. So the question changed. It
    // is no longer "is the verdict right" -- nothing here is entitled to an
    // opinion on that -- but "does the verdict REACH the caller", which is a
    // property of `check` and is not guaranteed by anything upstream. A `check`
    // that dropped the result, or looked at the wrong line, would still be
    // delegating to a correct parser.
    let clean = clean_row();
    assert!(
        serde_json::from_str::<serde_json::Value>(&clean).is_ok(),
        "the fixture must be valid JSON before corrupting it means anything"
    );

    // A corruption that destroys the LEADING BRACE is a different finding, and
    // both halves are asserted rather than one being waved through. `check`
    // selects the row by `starts_with('{')`, so such a line is not a malformed
    // row -- it is not a row, and the report has none. `Missing` is the right
    // answer there and `Malformed` would be the wrong one, because a survey's
    // question is "did this host report a row", not "was the text well-formed".
    // Measured: exactly the 8 corruptions of position 0, which is what makes the
    // two branches worth separating instead of accepting any defect at all.
    let mut escaped = Vec::new();
    let cases = corruptions(&clean);
    for (what, candidate) in &cases {
        if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
            continue;
        }
        let defects = check(&report_with(candidate));
        let expected = if candidate.starts_with('{') {
            defects
                .iter()
                .any(|defect| matches!(defect, RowDefect::Malformed { .. }))
        } else {
            defects.contains(&RowDefect::Missing)
        };
        if !expected {
            escaped.push(format!("{what}: got {defects:?}"));
        }
    }

    assert!(
        escaped.is_empty(),
        "{} of {} corruptions are unparseable and were not reported as the \
         defect they are:\n{}",
        escaped.len(),
        cases.len(),
        escaped.join("\n")
    );
}

#[test]
fn the_corruption_generator_reaches_defects_of_every_kind() {
    // **A generator that produced nothing, or only legal strings, would leave the
    // test above vacuous and green.** A generator cannot report the shape it
    // never reaches, so it has to be asked what it reached.
    let cases = corruptions(&clean_row());
    assert!(cases.len() > 500, "only {} corruptions", cases.len());

    let unparseable = cases
        .iter()
        .filter(|(_, candidate)| serde_json::from_str::<serde_json::Value>(candidate).is_err())
        .count();
    assert!(
        unparseable > 100,
        "only {unparseable} of {} corruptions are unparseable, so the wiring \
         test is mostly skipping its own body",
        cases.len()
    );

    let parseable = cases.len() - unparseable;
    assert!(
        parseable > 0,
        "every corruption is unparseable, so a `check` that reported `Malformed` \
         unconditionally would satisfy the wiring test"
    );
}
