// Copyright (c) 2026 Mike Grier

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use super::{Invocation, MAX_TIMEOUT_MS, UsageError, flags, parse};

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// A complete command line, with `extra` spliced in before the separator.
fn full(extra: &[&str], command: &[&str]) -> Vec<OsString> {
    let mut v = os(&[
        flags::TIMEOUT_MS,
        "1500",
        flags::STDOUT,
        "out.txt",
        flags::STDERR,
        "err.txt",
        flags::RESULT,
        "result.json",
    ]);
    v.extend(os(extra));
    v.push(OsString::from(flags::SEPARATOR));
    v.extend(os(command));
    v
}

fn expected(trace: bool, program: &str, args: &[&str]) -> Invocation {
    Invocation {
        timeout_ms: 1500,
        stdout: PathBuf::from("out.txt"),
        stderr: PathBuf::from("err.txt"),
        result: PathBuf::from("result.json"),
        trace,
        program: OsString::from(program),
        args: os(args),
    }
}

#[test]
fn a_full_command_line_parses() {
    assert_eq!(
        parse(full(&[], &["cargo", "test", "-p", "x"])),
        Ok(expected(false, "cargo", &["test", "-p", "x"]))
    );
}

#[test]
fn trace_is_off_unless_asked_for() {
    assert!(!parse(full(&[], &["p"])).unwrap().trace);
    assert!(parse(full(&[flags::TRACE], &["p"])).unwrap().trace);
}

#[test]
fn flags_may_come_in_any_order() {
    let args = os(&[
        flags::RESULT,
        "result.json",
        flags::TRACE,
        flags::STDERR,
        "err.txt",
        flags::TIMEOUT_MS,
        "1500",
        flags::STDOUT,
        "out.txt",
        flags::SEPARATOR,
        "p",
    ]);
    assert_eq!(parse(args), Ok(expected(true, "p", &[])));
}

#[test]
fn a_program_with_no_arguments_has_none() {
    assert_eq!(
        parse(full(&[], &["p"])).unwrap().args,
        Vec::<OsString>::new()
    );
}

#[test]
fn arguments_after_the_separator_are_the_commands_even_when_they_look_like_flags() {
    let parsed = parse(full(
        &[],
        &["p", flags::TRACE, flags::TIMEOUT_MS, "--", "x"],
    ))
    .unwrap();
    assert!(
        !parsed.trace,
        "a --trace after the separator is the command's"
    );
    assert_eq!(parsed.timeout_ms, 1500);
    assert_eq!(
        parsed.args,
        os(&[flags::TRACE, flags::TIMEOUT_MS, "--", "x"])
    );
}

#[test]
fn paths_with_spaces_are_kept_whole() {
    let mut args = full(&[], &["C:\\Program Files\\x.exe", "a b"]);
    args[3] = OsString::from("C:\\some dir\\out.txt");
    let parsed = parse(args).unwrap();
    assert_eq!(parsed.stdout, PathBuf::from("C:\\some dir\\out.txt"));
    assert_eq!(parsed.program, OsString::from("C:\\Program Files\\x.exe"));
    assert_eq!(parsed.args, os(&["a b"]));
}

#[test]
fn a_value_is_taken_literally_even_when_it_looks_like_a_flag() {
    // A path named `--trace` is a strange path, not a request to trace.
    let mut args = full(&[], &["p"]);
    args[3] = OsString::from(flags::TRACE);
    let parsed = parse(args).unwrap();
    assert_eq!(parsed.stdout, PathBuf::from(flags::TRACE));
    assert!(!parsed.trace);
}

#[test]
fn a_command_argument_that_is_not_unicode_passes_through() {
    let lone_surrogate = OsString::from_wide(&[0xD800]);
    let mut args = full(&[], &["p"]);
    args.push(lone_surrogate.clone());
    assert_eq!(parse(args).unwrap().args, vec![lone_surrogate]);
}

#[test]
fn the_timeout_accepts_its_whole_range() {
    for (text, ms) in [
        ("1", 1),
        ("15000", 15_000),
        (&MAX_TIMEOUT_MS.to_string()[..], MAX_TIMEOUT_MS),
    ] {
        let mut args = full(&[], &["p"]);
        args[1] = OsString::from(text);
        assert_eq!(parse(args).unwrap().timeout_ms, ms, "{text}");
    }
}

#[test]
fn the_timeout_refuses_zero_infinite_and_non_numbers() {
    for text in [
        "0",
        &u32::MAX.to_string()[..],
        "4294967296",
        "-1",
        "1.5",
        "",
        "15s",
        " 15",
    ] {
        let mut args = full(&[], &["p"]);
        args[1] = OsString::from(text);
        assert_eq!(
            parse(args),
            Err(UsageError::BadTimeout(OsString::from(text))),
            "{text}"
        );
    }
}

#[test]
fn a_timeout_that_is_not_unicode_is_refused_as_a_timeout() {
    let mut args = full(&[], &["p"]);
    args[1] = OsString::from_wide(&[0xD800]);
    assert!(matches!(parse(args), Err(UsageError::BadTimeout(_))));
}

#[test]
fn each_required_flag_is_required() {
    for (index, flag) in [
        (0, flags::TIMEOUT_MS),
        (2, flags::STDOUT),
        (4, flags::STDERR),
        (6, flags::RESULT),
    ] {
        let mut args = full(&[], &["p"]);
        args.drain(index..index + 2);
        assert_eq!(parse(args), Err(UsageError::Missing(flag)), "{flag}");
    }
}

#[test]
fn a_repeated_flag_is_refused() {
    for repeat in [
        &[flags::TIMEOUT_MS, "9"][..],
        &[flags::STDOUT, "o"],
        &[flags::STDERR, "e"],
        &[flags::RESULT, "r"],
        &[flags::TRACE, flags::TRACE],
    ] {
        let flag = repeat[0];
        assert!(
            matches!(parse(full(repeat, &["p"])), Err(UsageError::Repeated(f)) if f == flag),
            "{flag}"
        );
    }
}

#[test]
fn a_flag_with_no_value_at_the_end_is_refused() {
    for flag in [
        flags::TIMEOUT_MS,
        flags::STDOUT,
        flags::STDERR,
        flags::RESULT,
    ] {
        assert_eq!(
            parse(os(&[flag])),
            Err(UsageError::MissingValue(flag)),
            "{flag}"
        );
    }
}

#[test]
fn an_unknown_argument_before_the_separator_is_refused() {
    assert_eq!(
        parse(full(&["--timeout"], &["p"])),
        Err(UsageError::UnknownFlag(OsString::from("--timeout")))
    );
    assert_eq!(
        parse(os(&["cargo", "test"])),
        Err(UsageError::UnknownFlag(OsString::from("cargo")))
    );
}

#[test]
fn a_command_line_with_no_command_is_refused() {
    assert_eq!(parse(Vec::new()), Err(UsageError::NoProgram));
    let mut no_program = full(&[], &[]);
    assert_eq!(no_program.pop(), Some(OsString::from(flags::SEPARATOR)));
    assert_eq!(
        parse(no_program),
        Err(UsageError::NoProgram),
        "no separator"
    );
    assert_eq!(
        parse(full(&[], &[])),
        Err(UsageError::NoProgram),
        "nothing after it"
    );
}

#[test]
fn every_refusal_names_what_was_wrong() {
    let cases = [
        (UsageError::UnknownFlag(OsString::from("--x")), "--x"),
        (UsageError::MissingValue(flags::STDOUT), flags::STDOUT),
        (UsageError::Repeated(flags::TRACE), flags::TRACE),
        (UsageError::Missing(flags::RESULT), flags::RESULT),
        (
            UsageError::BadTimeout(OsString::from("0")),
            flags::TIMEOUT_MS,
        ),
        (UsageError::NoProgram, flags::SEPARATOR),
    ];
    for (error, named) in cases {
        let text = error.to_string();
        assert!(text.contains(named), "{text:?} should name {named:?}");
    }
}
