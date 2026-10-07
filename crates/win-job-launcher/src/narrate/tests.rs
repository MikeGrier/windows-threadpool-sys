// Copyright (c) 2026 Mike Grier

use std::io::{self, Write};

use super::{Narrator, PREFIX};

fn text(n: Narrator<Vec<u8>>) -> String {
    String::from_utf8(n.into_inner()).expect("the narration is UTF-8")
}

#[test]
fn trace_lines_are_dropped_unless_tracing() {
    let mut n = Narrator::new(Vec::new(), false);
    n.trace(format_args!("step"));
    assert_eq!(text(n), "");
}

#[test]
fn a_trace_line_carries_the_prefix_an_offset_and_the_message() {
    let mut n = Narrator::new(Vec::new(), true);
    n.trace(format_args!("spawned PID {}", 42));
    let out = text(n);
    let rest = out
        .strip_prefix(&format!("{PREFIX} +"))
        .unwrap_or_else(|| panic!("{out:?}"));
    let (seconds, message) = rest.split_once("s: ").unwrap_or_else(|| panic!("{out:?}"));
    let (whole, fraction) = seconds.split_once('.').unwrap_or_else(|| panic!("{out:?}"));
    assert!(whole.parse::<u64>().is_ok(), "{out:?}");
    assert_eq!(fraction.len(), 3, "milliseconds: {out:?}");
    assert_eq!(message, "spawned PID 42\n");
}

#[test]
fn errors_are_written_whether_or_not_tracing() {
    for trace in [false, true] {
        let mut n = Narrator::new(Vec::new(), trace);
        n.error(format_args!("no result"));
        assert_eq!(text(n), format!("{PREFIX}: error: no result\n"));
    }
}

#[test]
fn every_physical_line_of_a_multiline_error_carries_the_prefix() {
    let mut n = Narrator::new(Vec::new(), false);
    n.error(format_args!("bad flag\nusage: win-job-launcher ..."));
    assert_eq!(
        text(n),
        format!("{PREFIX}: error: bad flag\n{PREFIX}: usage: win-job-launcher ...\n")
    );
}

#[test]
fn every_physical_line_of_a_multiline_trace_carries_the_prefix_and_stamp() {
    let mut n = Narrator::new(Vec::new(), true);
    n.trace(format_args!("a\nb\r\nc"));
    let out = text(n);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out:?}");
    for (line, expected) in lines.iter().zip(["a", "b", "c"]) {
        assert!(line.starts_with(&format!("{PREFIX} +")), "{out:?}");
        assert!(line.ends_with(&format!("s: {expected}")), "{out:?}");
    }
}

#[test]
fn an_empty_message_is_still_one_prefixed_line() {
    let mut n = Narrator::new(Vec::new(), false);
    n.error(format_args!(""));
    assert_eq!(text(n), format!("{PREFIX}: error: \n"));
}

#[test]
fn a_trailing_newline_does_not_add_a_blank_prefixed_line() {
    let mut n = Narrator::new(Vec::new(), false);
    n.error(format_args!("one\n"));
    assert_eq!(text(n), format!("{PREFIX}: error: one\n"));
}

#[test]
fn each_call_is_one_line_in_order() {
    let mut n = Narrator::new(Vec::new(), true);
    n.trace(format_args!("one"));
    n.error(format_args!("two"));
    n.trace(format_args!("three"));
    let out = text(n);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "{out:?}");
    assert!(lines[0].ends_with(": one"));
    assert_eq!(lines[1], format!("{PREFIX}: error: two"));
    assert!(lines[2].ends_with(": three"));
}

#[test]
fn the_offset_does_not_go_backwards() {
    let n = Narrator::new(Vec::new(), true);
    let first = n.elapsed();
    let second = n.elapsed();
    assert!(second >= first);
}

/// A writer that refuses everything.
struct Broken;

impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("refused"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("refused"))
    }
}

#[test]
fn a_writer_that_fails_does_not_stop_the_narrator() {
    let mut n = Narrator::new(Broken, true);
    n.trace(format_args!("lost"));
    n.error(format_args!("also lost"));
    // Reaching here is the assertion: neither call panicked or returned early.
    let _ = n.into_inner();
}
