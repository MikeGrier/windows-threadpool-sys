// Copyright (c) 2026 Mike Grier
//! Proof that the guard allocator **fires**.
//!
//! Every other test in this crate asserts structure -- where the guard page
//! sits, what protection a freed range carries. None of them prove the
//! structure has any consequence, because an access violation cannot be caught
//! in-process: it terminates the process rather than unwinding. So the
//! violations run in a subprocess (`src/bin/violate.rs`) and this test reads
//! the exit code.
//!
//! That distinction is the point of the file. An instrument that has never
//! been seen to fire is indistinguishable from one that cannot, and this
//! repository has already shipped a defect that a passing test suite declared
//! absent. These cases are the calibration.

use std::process::Command;

/// `STATUS_ACCESS_VIOLATION`, as a process exit code sees it.
const ACCESS_VIOLATION: i32 = -1_073_741_819; // 0xC0000005

/// The fixture's exit code for "the violation was not caught".
const UNDETECTED: i32 = 42;

fn run(mode: &str) -> (i32, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_violate"))
        .arg(mode)
        .output()
        .expect("run the violation fixture");
    let code = output.status.code().unwrap_or(i32::MIN);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    (code, text)
}

#[test]
fn a_clean_run_exits_successfully() {
    // The control, and it is load-bearing: without it, a fault in every other
    // case would be equally consistent with the allocator simply not working.
    let (code, out) = run("clean");
    assert_eq!(code, 0, "a program that breaks no rule must run normally");
    assert!(
        out.contains("clean: sum="),
        "the fixture did its work: {out}"
    );
}

#[test]
fn a_use_after_free_faults_instead_of_reading_stale_bytes() {
    // Issue #48's shape. On the system allocator this same code reads a
    // plausible byte and exits 0 -- measured, not assumed -- which is exactly
    // how that defect survived its own test suite.
    let (code, out) = run("uaf");
    assert_ne!(code, UNDETECTED, "the use-after-free was not caught: {out}");
    assert_eq!(
        code, ACCESS_VIOLATION,
        "expected STATUS_ACCESS_VIOLATION, got {code:#x}: {out}"
    );
    assert!(
        !out.contains("UNDETECTED"),
        "the fixture reached its post-violation print: {out}"
    );
}

#[test]
fn a_one_byte_read_past_the_end_faults() {
    // Proves the *right-alignment*, not merely the guard page's existence: a
    // guard page at the end of a page-rounded block would leave slack that a
    // one-byte overrun sails through.
    let (code, out) = run("overrun");
    assert_ne!(code, UNDETECTED, "the overrun was not caught: {out}");
    assert_eq!(
        code, ACCESS_VIOLATION,
        "expected STATUS_ACCESS_VIOLATION, got {code:#x}: {out}"
    );
}

#[test]
fn a_one_byte_write_past_the_end_faults() {
    // A read and a write into a guard page are different access types; proving
    // one says nothing about the other.
    let (code, out) = run("overrun-write");
    assert_ne!(
        code, UNDETECTED,
        "the overrunning write was not caught: {out}"
    );
    assert_eq!(
        code, ACCESS_VIOLATION,
        "expected STATUS_ACCESS_VIOLATION, got {code:#x}: {out}"
    );
}

#[test]
fn an_overrun_past_a_multi_page_allocation_faults() {
    // The case where recovering the reservation base by masking the pointer to
    // a page boundary would have been wrong: for an allocation larger than a
    // page, the pointer is not in the first page.
    let (code, out) = run("overrun-large");
    assert_ne!(code, UNDETECTED, "the large overrun was not caught: {out}");
    assert_eq!(
        code, ACCESS_VIOLATION,
        "expected STATUS_ACCESS_VIOLATION, got {code:#x}: {out}"
    );
}
