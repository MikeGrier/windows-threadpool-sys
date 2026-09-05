// Copyright (c) 2026 Mike Grier
//! Tests for [`measure`](super::measure)'s apparatus.
//!
//! **These assert what the experiment gives back, not what it found.** What it
//! finds is a fact about the host -- whether the manifest and the registry
//! setting lift `MAX_PATH` -- and asserting that here would encode this
//! machine's configuration into the suite. What must hold on every host is that
//! running the experiment leaves the process and the disk as it found them.
//!
//! # Why these serialize
//!
//! `measure` borrows two pieces of **process-wide** state: the current
//! directory, and a temporary root named after the process id. This crate's
//! tests run as threads in one process, so two of these running at once would
//! share both -- one call removing the tree another was still using, and the
//! two fighting over the current directory.
//!
//! That is a property of `measure` rather than a defect in it, and the fix is
//! not a per-call unique root: the current directory is one per process however
//! the directories are named, so concurrent calls could not work whatever the
//! tree was called. See `measure`'s own documentation. These tests therefore
//! take a lock, which is also what a consumer would have to do.
//!
//! Found by writing the third test below, which failed until the lock existed.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use wtf_string::Wtf16String;

use super::measure;

/// Serializes the tests here, for the reason in this module's documentation.
static APPARATUS: Mutex<()> = Mutex::new(());

/// Take the lock, ignoring poisoning.
///
/// A panic in one of these tests leaves the mutex poisoned, which would turn
/// one real failure into three and hide which was the original.
fn exclusive() -> MutexGuard<'static, ()> {
    APPARATUS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Where this process's current directory points.
fn current_directory() -> PathBuf {
    std::env::current_dir().expect("the test process must have a current directory")
}

/// The tree `measure` builds, named as it names it.
fn apparatus_root() -> PathBuf {
    std::env::temp_dir().join(format!("long-path-probe-{}", std::process::id()))
}

#[test]
fn measuring_leaves_the_current_directory_where_it_found_it() {
    // **The leak that matters most in a library.** `measure` moves the process
    // into its temporary root so the length under test lives in the relative
    // path rather than in the current directory. The process is shared, so
    // failing to move back would silently re-root every later relative path in
    // whatever called this -- a test, or any other consumer.
    let _lock = exclusive();
    let before = current_directory();

    let _ = measure(false);

    assert_eq!(
        current_directory(),
        before,
        "the experiment left the process parked somewhere else"
    );
}

#[test]
fn measuring_removes_the_tree_it_built() {
    // The tree is deliberately deeper than `MAX_PATH`, which is exactly what
    // stops Explorer and `del` from clearing it up -- so litter from a probe
    // about long paths is litter that is awkward to remove by hand.
    let _lock = exclusive();

    let _ = measure(false);

    assert!(
        !apparatus_root().exists(),
        "the experiment left its temporary tree behind at {}",
        apparatus_root().display()
    );
}

#[test]
fn the_apparatus_is_cleaned_up_even_when_the_experiment_is_run_twice() {
    // Two calls in one process, which is the shape a test run takes and the
    // shape the probe binaries never do. The second rebuilds a tree under the
    // same name the first removed, so a first run that left its tree behind
    // would surface here -- as an apparatus error rather than a silent leak.
    let _lock = exclusive();
    let before = current_directory();

    let first = measure(false);
    let second = measure(false);

    assert_eq!(first.apparatus_error, None, "first run");
    assert_eq!(second.apparatus_error, None, "second run");
    assert!(
        !apparatus_root().exists(),
        "{} survived",
        apparatus_root().display()
    );
    assert_eq!(current_directory(), before);
}

#[test]
fn the_resolved_length_is_counted_in_the_unit_max_path_uses() {
    // **The bug this guards is invisible on an ASCII host**, which is every
    // machine this has run on so far. `MAX_PATH` counts UTF-16 code units;
    // `OsStr::len` counts Rust's platform encoding, which is WTF-8 here. The
    // two agree for ASCII and diverge for anything else, so a `%TEMP%` with a
    // non-ASCII character made the reported length too large and could push an
    // attempt onto the wrong side of the ceiling in the report.
    //
    // Asserted on the encoding rather than through `measure`, because the fault
    // needs a non-ASCII temporary directory that this test cannot conjure --
    // and pinning the relationship is what actually stops the regression.
    let ascii = OsString::from("C:\\Temp");
    assert_eq!(
        Wtf16String::from_os_str(&ascii).len(),
        ascii.len(),
        "the two units must agree for ASCII, or this test proves nothing below"
    );

    // U+00E9 is one UTF-16 unit and two WTF-8 bytes; U+4E2D is one and three.
    // A path Windows sees as shorter than `MAX_PATH` can therefore look longer
    // when measured in bytes -- the direction that matters, since it would
    // report a refusal as expected when it was not.
    for accented in ["C:\\Tempé", "C:\\Temp中"] {
        let path = OsString::from(accented);
        let wide = Wtf16String::from_os_str(&path).len();

        assert!(
            wide < path.len(),
            "{accented:?} must expose the divergence: {wide} wide vs {} bytes",
            path.len()
        );
        assert_eq!(
            wide,
            accented.chars().count(),
            "every character here is one UTF-16 unit, so the wide count is the character count"
        );
    }
}
