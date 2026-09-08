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
use std::os::windows::ffi::OsStringExt as _;
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
    // shape the probe binaries never do.
    //
    // A leak from the first run does *not* surface as an apparatus error, and it
    // is worth saying so: `create_dir_verbatim` treats `ERROR_ALREADY_EXISTS` as
    // success and `create_target` opens with `CREATE_ALWAYS`, so a surviving tree
    // is silently reused and the second run reports itself perfectly healthy. The
    // assertion that does the work here is the `exists()` check below -- the
    // second `apparatus_error` is a control on the run itself, not the leak
    // detector.
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

#[test]
fn an_over_long_tmp_is_refused_before_the_call_that_would_hang() {
    // The limit is a flat number, so the test is just that it is applied where it
    // says: at the limit run, one past it refuse.
    let ok = OsString::from("c".repeat(super::MAX_TEMP_DIR));
    assert!(
        super::temp_dir_refusal(Some(&ok), None).is_none(),
        "the limit itself must run"
    );

    let bad = OsString::from("c".repeat(super::MAX_TEMP_DIR + 1));
    let refusal = super::temp_dir_refusal(Some(&bad), None).expect("one past the limit refuses");
    assert!(
        refusal.contains("%TMP%") && refusal.contains(&(super::MAX_TEMP_DIR + 1).to_string()),
        "the refusal must name the variable and its length, or it cannot be acted on: {refusal}"
    );
}

#[test]
fn temp_is_only_consulted_when_tmp_is_unset() {
    // `GetTempPath` reads `%TMP%` first, so a short `%TMP%` decides the run and a
    // long `%TEMP%` beside it is never looked at. Refusing on it would turn a
    // perfectly usable configuration into a false apparatus error.
    let short = OsString::from("c".repeat(20));
    let long = OsString::from("c".repeat(super::MAX_TEMP_DIR + 1));

    assert!(
        super::temp_dir_refusal(Some(&short), Some(&long)).is_none(),
        "a usable %TMP% wins, whatever %TEMP% says"
    );
    assert!(
        super::temp_dir_refusal(None, Some(&long)).is_some(),
        "with %TMP% unset, %TEMP% is what GetTempPath uses and what can hang"
    );
    assert!(
        super::temp_dir_refusal(None, None).is_none(),
        "with neither set the fallbacks are short and the call returns"
    );
}

#[test]
fn the_refusal_length_is_counted_in_utf16_units() {
    // Counting Rust's platform encoding instead would disagree the moment a
    // non-ASCII character appeared, and disagreeing here means either a false
    // refusal or a real hang.
    //
    // The fixture is chosen so every reading gives a different answer, and only
    // one of them is right: 200 characters outside the BMP are 200 chars (which
    // would be allowed, since the limit is 200), 400 UTF-16 units (refused, and
    // correct), and 800 bytes of WTF-8.
    let astral = OsString::from("\u{1F600}".repeat(super::MAX_TEMP_DIR));
    assert_eq!(
        Wtf16String::from_os_str(&astral).len(),
        400,
        "the fixture must be over the limit in UTF-16 units specifically"
    );

    let refusal =
        super::temp_dir_refusal(Some(&astral), None).expect("400 UTF-16 units is over the limit");
    assert!(
        refusal.contains("400"),
        "the length reported must be the one Windows counts: {refusal}"
    );
}

#[test]
fn a_unc_temp_dir_is_refused_rather_than_spelled_wrong() {
    // The apparatus composes `\\?\` by concatenation, and a UNC root needs
    // `\\?\UNC\server\share` rather than `\\?\` glued onto `\\server`. Gluing
    // produces a malformed path, so the run would fail in the apparatus and
    // report nothing about the ceiling.
    let unc = OsString::from(r"\\server\share\temp");
    let refusal = super::temp_dir_refusal(Some(&unc), None).expect("a UNC root is refused");

    assert!(
        refusal.contains("UNC") && refusal.contains("%TMP%"),
        "the refusal must name the shape and the variable: {refusal}"
    );

    // The control: an ordinary local path with the same prefix character must
    // still run. `\` alone is not UNC, and refusing it would reject a rooted
    // path on the current drive.
    let rooted = OsString::from(r"\temp");
    assert!(
        super::temp_dir_refusal(Some(&rooted), None).is_none(),
        "a single leading separator is a rooted local path, not a UNC root"
    );
}

#[test]
fn a_temp_dir_that_is_not_well_formed_utf16_is_refused() {
    // `Path::display` substitutes U+FFFD for an unpaired surrogate, so composing
    // the apparatus's verbatim paths as text would name a different file --
    // silently, and in the apparatus rather than in the measurement.
    //
    // 0xD800 is a lone high surrogate: valid in a Windows path, not expressible
    // as a Rust `str`.
    let ill_formed = OsString::from_wide(&[0x0043, 0x003A, 0x005C, 0xD800]);
    assert!(
        ill_formed.to_str().is_none(),
        "the fixture must actually be ill-formed, or the test proves nothing"
    );

    let refusal =
        super::temp_dir_refusal(Some(&ill_formed), None).expect("an ill-formed path is refused");
    assert!(
        refusal.contains("well-formed") && refusal.contains("%TMP%"),
        "the refusal must say what is wrong with it: {refusal}"
    );
}

#[test]
fn a_device_namespace_temp_dir_is_refused_as_itself_and_not_as_unc() {
    // `\\?\` and `\\.\` open with the two backslashes a UNC root does, so a
    // prefix test written for UNC claims them too. The refusal would then tell
    // someone with a perfectly local temporary directory that it is a server
    // share, which is the defect this whole probe is written against: a report
    // asserting something the run never established.
    for prefix in [r"\\?\", r"\\.\"] {
        let value = OsString::from(format!(r"{prefix}C:\Temp"));
        let refusal = super::temp_dir_refusal(Some(&value), None)
            .unwrap_or_else(|| panic!("{prefix} is refused"));

        assert!(
            refusal.contains(prefix),
            "the refusal must name the prefix it actually found: {refusal}"
        );
        assert!(
            !refusal.contains("UNC"),
            "a device-namespace path is not a UNC root and must not be called one: {refusal}"
        );
    }
}

#[test]
fn an_ill_formed_temp_dir_is_classified_before_its_prefix_is_read() {
    // Ordering, pinned: the prefix tests read the value as text, and reading an
    // ill-formed value as text is the exact substitution the well-formedness
    // refusal exists to prevent. Classifying first would report this value under
    // whichever prefix its replacement characters happened to spell -- here,
    // UNC -- and the explanation would be about the wrong thing.
    let ill_formed = OsString::from_wide(&[0x005C, 0x005C, 0x0073, 0xD800]);
    assert!(
        ill_formed.to_str().is_none(),
        "the fixture must actually be ill-formed, or the test proves nothing"
    );

    let refusal = super::temp_dir_refusal(Some(&ill_formed), None)
        .expect("an ill-formed path is refused whatever it starts with");
    assert!(
        refusal.contains("well-formed"),
        "the refusal must be about the encoding, not the prefix: {refusal}"
    );
    assert!(
        !refusal.contains("UNC"),
        "an ill-formed value must not be classified by text it cannot be read as: {refusal}"
    );
}

/// What `reg query HKLM\...\FileSystem /v LongPathsEnabled` prints, verbatim
/// apart from the value, which is the part under test.
fn reg_output(value: &str) -> String {
    format!(
        "\r\n\
         HKEY_LOCAL_MACHINE\\SYSTEM\\CurrentControlSet\\Control\\FileSystem\r\n\
             LongPathsEnabled    REG_DWORD    {value}\r\n\r\n"
    )
}

#[test]
fn the_registry_value_is_read_as_a_number_and_not_as_a_substring() {
    // The reading this replaces tested the output for the substring `0x1`. The
    // failure that costs something is the false negative: every nonzero value
    // that does not happen to begin with a 1 read as disabled, so a machine set
    // to `0x2` would be reported as not opted in and the run's conclusion would
    // invert. `0x2` is therefore the case that matters here.
    assert!(
        super::enabled_in(&reg_output("0x2")),
        "any nonzero value is the flag set, not just the ones spelled with a 1"
    );
    assert!(super::enabled_in(&reg_output("0x1")), "0x1 is enabled");
    assert!(!super::enabled_in(&reg_output("0x0")), "0x0 is disabled");
}

#[test]
fn a_registry_answer_that_is_not_the_queried_value_reads_as_disabled() {
    // Absent, wrong type, and unparsable all mean the same thing here, and it is
    // the same thing a missing key means: not opted in. The point of parsing the
    // whole line is that a value belonging to something else can no longer be
    // mistaken for this one.
    for stdout in [
        String::new(),
        "    SomethingElse    REG_DWORD    0x1\r\n".to_string(),
        "    LongPathsEnabled    REG_SZ    0x1\r\n".to_string(),
        "    LongPathsEnabled    REG_DWORD    enabled\r\n".to_string(),
        "    LongPathsEnabled    REG_DWORD\r\n".to_string(),
    ] {
        assert!(
            !super::enabled_in(&stdout),
            "only the queried DWORD may report the machine as opted in: {stdout:?}"
        );
    }
}
