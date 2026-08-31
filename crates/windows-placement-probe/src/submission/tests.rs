// Copyright (c) 2026 Mike Grier
//! Tests for the paste-ready submission.

use super::{DISCUSSION_URL, checksum, file_name, render_submission};
use crate::record::tests::fully_populated;

#[test]
fn the_submission_carries_its_own_markdown_fences() {
    // The whole friction argument rests on this: a runner who has never thought
    // about markdown must be able to select all, copy, paste, and have it
    // render as a code block.
    let text = render_submission(&fully_populated()).expect("must render");

    assert!(text.contains("```text\n"), "no opening fence");
    assert!(text.trim_end().ends_with("```"), "no closing fence");
    assert_eq!(
        text.matches("```").count(),
        2,
        "exactly one fenced block, or the paste will render half as prose"
    );
}

#[test]
fn the_submission_names_where_to_send_it() {
    // An instruction that lives only in a README is one that half of them will
    // not have read.
    let text = render_submission(&fully_populated()).expect("must render");

    assert!(text.contains(DISCUSSION_URL), "got {text}");
    assert!(
        text.lines()
            .next()
            .is_some_and(|line| line.contains("Paste")),
        "the instruction must be the first thing on screen"
    );
}

#[test]
fn the_submission_contains_both_the_report_and_the_record() {
    // Everything needed on screen: a submission that requires the sender to
    // find a file will sometimes arrive without it.
    let record = fully_populated();
    let text = render_submission(&record).expect("must render");

    assert!(
        text.contains("what does thread placement cost"),
        "the human report is missing"
    );
    assert!(
        text.contains("\"schema_version\""),
        "the machine-readable record is missing"
    );
    assert!(
        text.contains("Example CPU"),
        "the machine description is missing"
    );
}

#[test]
fn the_checksum_is_printed_and_matches_the_json_that_follows_it() {
    // Guards the thing that makes the checksum worth having: it must be a
    // digest of what was actually emitted, not of something adjacent.
    let record = fully_populated();
    let text = render_submission(&record).expect("must render");

    let json = serde_json::to_string_pretty(&record).expect("must serialize");
    let expected = checksum(json.as_bytes());

    assert!(
        text.contains(&expected),
        "the printed checksum does not match the emitted JSON"
    );
    assert!(
        text.contains(&json),
        "the JSON in the text is not the record"
    );
}

#[test]
fn the_checksum_changes_when_the_record_does() {
    // A digest that did not move with its input would be worse than none, since
    // a reader would trust it.
    let a = fully_populated();
    let mut b = fully_populated();
    b.placements[0].nanos_per_item = 42.0;

    let json_a = serde_json::to_string_pretty(&a).expect("must serialize");
    let json_b = serde_json::to_string_pretty(&b).expect("must serialize");

    assert_ne!(checksum(json_a.as_bytes()), checksum(json_b.as_bytes()));
}

#[test]
fn the_checksum_catches_a_truncated_paste() {
    // The failure actually being defended against: a scrollback limit or a
    // half-dragged selection.
    let json = serde_json::to_string_pretty(&fully_populated()).expect("must serialize");
    let truncated = &json[..json.len() / 2];

    assert_ne!(checksum(json.as_bytes()), checksum(truncated.as_bytes()));
}

#[test]
fn the_checksum_is_stable_for_identical_input() {
    let bytes = b"placement";

    assert_eq!(checksum(bytes), checksum(bytes));
    assert_eq!(checksum(bytes).len(), 16, "sixteen hex characters");
    assert!(checksum(bytes).chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn the_checksum_is_described_as_not_a_security_control() {
    // A digest printed without qualification invites a reader to assume it
    // proves more than it does.
    let text = render_submission(&fully_populated()).expect("must render");

    assert!(
        text.contains("not a security control"),
        "the checksum must not overstate what it establishes"
    );
}

#[test]
fn the_human_report_stays_within_a_narrow_terminal() {
    // Not producing an overlong line beats detecting a reflowed one, and this
    // half of the output is entirely under our control -- every line is
    // something the report chose to lay out that way.
    let text = crate::report::render(&fully_populated());

    for line in text.lines() {
        assert!(
            line.chars().count() <= 100,
            "a {}-character report line will wrap on a normal terminal: {line:?}",
            line.chars().count()
        );
    }
}

#[test]
fn the_whole_submission_stays_within_a_wide_terminal() {
    // A looser bound, and deliberately so: the JSON's own lines carry string
    // *values* -- a slice names two processors with five fields each -- and
    // those cannot be shortened without removing information the record exists
    // to carry. 120 is the width this settles on, with the human half held to
    // the stricter bound above.
    let text = render_submission(&fully_populated()).expect("must render");

    for line in text.lines() {
        assert!(
            line.chars().count() <= 120,
            "a {}-character line is long enough to be reflowed on paste: {line:?}",
            line.chars().count()
        );
    }
}

#[test]
fn the_file_name_is_predictable_and_safe_for_a_filesystem() {
    let record = fully_populated();
    let name = file_name(&record);

    assert!(name.starts_with("placement-probe-v1-"), "got {name}");
    assert!(name.ends_with(".json"), "got {name}");
    assert!(
        !name.contains(':'),
        "a colon is not valid in a Windows file name: {name}"
    );
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.'),
        "got {name}"
    );
}

#[test]
fn two_runs_do_not_collide_on_one_file_name() {
    // Overwriting a previous result silently is a data loss nobody notices.
    let first = fully_populated();
    let mut second = fully_populated();
    second.recorded_at = "2026-09-01T13:00:00Z".to_owned();

    assert_ne!(file_name(&first), file_name(&second));
}
