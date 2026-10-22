// Copyright (c) 2026 Mike Grier
//! What a debugger receives (WD-D-2): this test binary starts itself as a process being debugged, runs
//! `a_process_that_sends_strings` in it, and keeps each string the process sends, through the crate's `test-util`
//! debugger (WD-D-3), which uses only documented debugger functions.
//!
//! This is evidence of what was observed, written as a test so that it is observed again: the crate's
//! contract is the call's page, which says the string is displayed "by the debugger", not how it arrives.

#![cfg(windows)]

use std::time::Duration;

use win_debugging_sys::{Captured, output_debug_str, output_debug_string, run_debugged};

/// The name of the test the process being debugged runs.
const SENDER: &str = "a_process_that_sends_strings";

/// How long the whole exchange may take. Generous: the process starts, sends a handful of strings, and exits.
const BOUND: Duration = Duration::from_secs(30);

/// The length of the string that is too long for the debugger's event to hold whole.
const LONG: usize = 100_000;

/// What the process sends, in order, before the long string.
fn cases() -> Vec<Vec<u16>> {
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    vec![
        units("win-debugging-sys: one"),
        Vec::new(),
        units("two words, and punctuation; \u{e9}"),
        units("\u{3042}\u{1f600}"),
        vec![0xd800, 0x61],
    ]
}

/// The string the process sends last, too long for the event to describe whole.
fn long() -> Vec<u16> {
    vec![u16::from(b'L'); LONG]
}

/// Run in the process being debugged: sends the strings. Run on its own, with no debugger, the call does
/// nothing, so this is harmless in an ordinary run of the tests.
#[test]
fn a_process_that_sends_strings() {
    for case in cases() {
        output_debug_string(&case).expect("no NUL");
    }
    // Refused, so nothing of it is sent, not even what comes before the NUL.
    assert!(output_debug_str("cut\0short").is_err());
    assert!(output_debug_string(&[0x61, 0, 0x62]).is_err());
    output_debug_string(&long()).expect("no NUL");
}

/// Starts this test binary running `SENDER` as a process being debugged, and keeps what it sends.
fn debug_the_sender() -> Captured {
    let exe = std::env::current_exe().expect("the test binary's path");
    run_debugged(&exe, &["--exact", SENDER, "--test-threads=1"], BOUND)
        .expect("the process was debugged")
}

#[test]
fn a_debugger_receives_each_string_the_process_sends_and_none_that_was_refused() {
    let observed = debug_the_sender();
    assert_eq!(observed.exit_code, Some(0), "the sender did not pass");
    let expected = cases();
    assert_eq!(
        observed.strings.len(),
        expected.len() + 1,
        "one event for each string sent, and none for the two refused"
    );
    // Observed: a debugger that waits with `WaitForDebugEventEx` is sent each string as Unicode, as the page
    // says it will be, with every unit as sent, an unpaired surrogate included.
    for (received, sent) in observed.strings.iter().zip(&expected) {
        assert!(received.unicode, "{sent:?} arrived as ANSI");
        assert_eq!(&received.units, sent);
    }
}

#[test]
fn a_string_too_long_for_the_event_arrives_cut_at_the_length_the_page_says() {
    let observed = debug_the_sender();
    let received = observed.strings.last().expect("the long string's event");
    assert!(received.unicode);
    // The page for the structure: the length field holds "the lower 16 bits of the length of the string in
    // bytes", and the string's length counts the terminator.
    let sent_bytes = (long().len() + 1) * size_of::<u16>();
    let kept_bytes = sent_bytes % (1 << 16);
    assert_eq!(received.units.len() * size_of::<u16>(), kept_bytes);
    assert!(received.units.iter().all(|&unit| unit == u16::from(b'L')));
}

#[test]
fn a_program_that_is_not_there_is_an_error_and_not_a_capture() {
    let missing = std::path::Path::new("Q:\\no\\such\\program.exe");
    assert!(run_debugged(missing, &[], Duration::from_secs(1)).is_err());
}

#[test]
fn a_process_that_outlives_the_bound_is_ended_and_has_no_exit_code() {
    let exe = std::env::current_exe().expect("the test binary's path");
    let observed = run_debugged(
        &exe,
        &[
            "--exact",
            "a_process_that_waits",
            "--ignored",
            "--test-threads=1",
        ],
        Duration::from_millis(1_500),
    )
    .expect("the process was debugged");
    assert_eq!(observed.exit_code, None);
}

/// Run in the process being debugged, only by the test above: it outlives that test's bound.
#[test]
#[ignore = "run by a_process_that_outlives_the_bound_is_ended_and_has_no_exit_code, in a process of its own"]
fn a_process_that_waits() {
    std::thread::sleep(Duration::from_secs(60));
}
