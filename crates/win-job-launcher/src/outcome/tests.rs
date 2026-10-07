// Copyright (c) 2026 Mike Grier

use std::fs;

use super::{Accounting, Outcome, Report, write_result};

const ACCOUNTING: Accounting = Accounting {
    total_processes: 3,
    active_processes: 1,
    user_cpu_ms: 12_345,
    kernel_cpu_ms: 67,
};

fn report(outcome: Outcome, accounting: Option<Accounting>) -> String {
    Report {
        outcome,
        elapsed_ms: 1500,
        accounting,
    }
    .to_json()
}

fn not_started(error: &str) -> String {
    report(
        Outcome::NotStarted {
            error: error.to_owned(),
            os_error: None,
        },
        None,
    )
}

#[test]
fn an_exit_reports_its_code_and_strays() {
    assert_eq!(
        report(Outcome::Exited { code: 0, strays: 0 }, Some(ACCOUNTING)),
        r#"{"outcome":"exited","code":0,"strays":0,"elapsedMs":1500,"totalProcesses":3,"activeProcesses":1,"userCpuMs":12345,"kernelCpuMs":67}"#
    );
}

#[test]
fn an_ntstatus_exit_code_is_written_signed() {
    let json = report(
        Outcome::Exited {
            code: -1_073_741_502,
            strays: 2,
        },
        None,
    );
    assert_eq!(
        json,
        r#"{"outcome":"exited","code":-1073741502,"strays":2,"elapsedMs":1500}"#
    );
}

#[test]
fn the_extreme_exit_codes_round_trip() {
    for code in [i32::MIN, -1, 1, 101, i32::MAX] {
        let json = report(Outcome::Exited { code, strays: 0 }, None);
        assert!(json.contains(&format!(r#""code":{code},"#)), "{json}");
    }
}

#[test]
fn a_timeout_reports_whether_the_kill_was_confirmed() {
    assert_eq!(
        report(Outcome::TimedOut { confirmed: true }, Some(ACCOUNTING)),
        r#"{"outcome":"timed-out","confirmed":true,"elapsedMs":1500,"totalProcesses":3,"activeProcesses":1,"userCpuMs":12345,"kernelCpuMs":67}"#
    );
    assert_eq!(
        report(Outcome::TimedOut { confirmed: false }, None),
        r#"{"outcome":"timed-out","confirmed":false,"elapsedMs":1500}"#
    );
}

#[test]
fn a_failure_to_start_reports_the_reason_and_the_os_error() {
    let json = report(
        Outcome::NotStarted {
            error: String::from("program not found"),
            os_error: Some(2),
        },
        Some(Accounting::default()),
    );
    assert_eq!(
        json,
        r#"{"outcome":"not-started","error":"program not found","osError":2,"elapsedMs":1500,"totalProcesses":0,"activeProcesses":0,"userCpuMs":0,"kernelCpuMs":0}"#
    );
}

#[test]
fn a_missing_os_error_is_null_rather_than_absent() {
    assert_eq!(
        not_started("no job"),
        r#"{"outcome":"not-started","error":"no job","osError":null,"elapsedMs":1500}"#
    );
}

#[test]
fn quotes_and_backslashes_are_escaped() {
    assert!(
        not_started(r#"C:\a "b""#).contains(r#""error":"C:\\a \"b\"""#),
        "{}",
        not_started(r#"C:\a "b""#)
    );
}

#[test]
fn control_characters_are_escaped() {
    let json = not_started("a\r\nb\tc\u{0}\u{7f}");
    assert!(
        json.contains(r#""error":"a\u000d\u000ab\u0009c\u0000\u007f""#),
        "{json}"
    );
}

#[test]
fn everything_outside_ascii_is_escaped_so_the_line_stays_7_bit() {
    // An accented letter, a CJK character, and one outside the BMP, which
    // JSON spells as a surrogate pair.
    let json = not_started("\u{e9}\u{4e2d}\u{1f600}");
    assert!(
        json.contains(r#""error":"\u00e9\u4e2d\ud83d\ude00""#),
        "{json}"
    );
    assert!(json.is_ascii());
}

#[test]
fn printable_ascii_passes_through_unchanged() {
    let printable: String = (' '..='~').filter(|c| *c != '"' && *c != '\\').collect();
    assert!(not_started(&printable).contains(&format!(r#""error":"{printable}""#)));
}

#[test]
fn every_report_is_one_line() {
    for outcome in [
        Outcome::Exited { code: 1, strays: 0 },
        Outcome::TimedOut { confirmed: true },
        Outcome::NotStarted {
            error: String::from("line one\nline two"),
            os_error: Some(5),
        },
    ] {
        let json = report(outcome, Some(ACCOUNTING));
        assert!(!json.contains('\n') && !json.contains('\r'), "{json}");
        assert!(json.starts_with('{') && json.ends_with('}'), "{json}");
    }
}

/// A scratch directory under the system temp directory, unique to one test.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "win-job-launcher-outcome-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

#[test]
fn the_result_file_holds_the_line_and_a_terminator_and_nothing_else() {
    let dir = scratch("written");
    let path = dir.join("result.json");
    let r = Report {
        outcome: Outcome::TimedOut { confirmed: true },
        elapsed_ms: 7,
        accounting: None,
    };
    write_result(&path, &r).expect("write the result");
    assert_eq!(fs::read_to_string(&path).unwrap(), r.to_json() + "\n");
    assert!(
        !dir.join("result.json.partial").exists(),
        "the temporary is renamed away"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_stale_result_is_replaced() {
    let dir = scratch("replaced");
    let path = dir.join("result.json");
    fs::write(&path, "stale").unwrap();
    let r = Report {
        outcome: Outcome::Exited { code: 0, strays: 0 },
        elapsed_ms: 1,
        accounting: None,
    };
    write_result(&path, &r).expect("write over the stale result");
    assert_eq!(fs::read_to_string(&path).unwrap(), r.to_json() + "\n");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_result_that_cannot_be_written_is_an_error_and_leaves_nothing() {
    let dir = scratch("unwritable");
    let path = dir.join("no such directory").join("result.json");
    let r = Report {
        outcome: Outcome::Exited { code: 0, strays: 0 },
        elapsed_ms: 1,
        accounting: None,
    };
    assert!(write_result(&path, &r).is_err());
    assert!(!path.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_rename_removes_its_temporary() {
    // A directory where the result should go makes the rename fail after the
    // temporary has been written.
    let dir = scratch("rename");
    let path = dir.join("result.json");
    fs::create_dir(&path).unwrap();
    let r = Report {
        outcome: Outcome::Exited { code: 0, strays: 0 },
        elapsed_ms: 1,
        accounting: None,
    };
    assert!(write_result(&path, &r).is_err());
    assert!(!dir.join("result.json.partial").exists());
    let _ = fs::remove_dir_all(&dir);
}
