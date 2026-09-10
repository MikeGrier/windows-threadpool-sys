// Copyright (c) Mike Grier.

//! Tests for the `GetFullPathNameW` entry.
//!
//! The negatives matter more than the positives here: this call does not verify
//! what it produces, and a suite that only ever resolved existing paths would
//! leave a reader believing it does.

use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use wtf_string::Wtf16String;

use super::{FullPathError, ResolveFullPath};
use crate::handle::tests::handle_allocation;
use crate::outcome::Win32Error;

fn resolve(path: &str) -> String {
    ResolveFullPath::new(Wtf16String::from(path))
        .perform()
        .expect("resolve the path")
        .to_string_lossy()
}

#[test]
fn dot_and_dotdot_components_are_resolved() {
    assert_eq!(
        resolve(r"C:\Windows\System32\..\.\Temp"),
        r"C:\Windows\Temp"
    );
}

#[test]
fn a_trailing_dot_component_is_resolved_away() {
    assert_eq!(resolve(r"C:\Windows\."), r"C:\Windows");
}

#[test]
fn forward_slashes_are_normalised_to_backslashes() {
    assert_eq!(resolve("C:/Windows/System32"), r"C:\Windows\System32");
}

#[test]
fn an_already_absolute_path_is_returned_unchanged() {
    assert_eq!(resolve(r"C:\Windows"), r"C:\Windows");
}

#[test]
fn a_path_that_does_not_exist_resolves_perfectly_happily() {
    // Resolution does not verify the result: no error, and no check that any
    // component exists. A consumer wanting a verified path wants an open plus
    // GetFinalPathNameByHandleW.
    assert_eq!(
        resolve(r"C:\no-such-directory\..\nothing-here.txt"),
        r"C:\nothing-here.txt"
    );
}

#[test]
fn a_drive_letter_is_never_expanded() {
    // The hazard this entry does NOT close, asserted so the documentation and
    // the behaviour cannot drift apart: the result still starts with the drive
    // letter, whose meaning depends on the logon session at open time.
    let resolved = resolve(r"C:\Windows");

    assert!(
        resolved.starts_with("C:"),
        "the drive letter survives resolution: {resolved}"
    );
    assert!(
        !resolved.starts_with(r"\\?\"),
        "and it is not turned into a verbatim or device path: {resolved}"
    );
}

#[test]
fn a_relative_path_resolves_against_the_current_directory() {
    // Which is exactly why this must happen where the caller is: the process
    // current directory is shared mutable state.
    let _allocating = handle_allocation()
        .read()
        .expect("the lock is not poisoned");

    let current = std::env::current_dir().expect("read the current directory");
    let resolved = resolve("some-relative-name.txt");

    let expected = current.join("some-relative-name.txt");
    assert_eq!(
        resolved,
        expected.to_str().expect("the current directory is UTF-8")
    );
}

#[test]
fn a_deeply_nested_path_beyond_the_first_attempt_still_resolves() {
    // Forces the grow-the-buffer retry rather than assuming it works from
    // paths that always fitted the first attempt.
    let segment = "a-directory-with-a-deliberately-long-name";
    let mut path = String::from(r"C:\");
    for index in 0..12 {
        path.push_str(&format!("{segment}-{index:02}\\"));
    }
    path.push_str("file.txt");

    let resolved = resolve(&path);

    assert!(
        resolved.encode_utf16().count() > 260,
        "the fixture must exceed the first attempt: {} UTF-16 units",
        resolved.encode_utf16().count()
    );
    assert!(resolved.ends_with("file.txt"), "unexpected: {resolved}");
}

#[test]
fn an_empty_path_reports_the_raw_code() {
    let outcome = ResolveFullPath::new(Wtf16String::new()).perform();

    // Specifically the Win32 variant, not merely "an error". The point of the
    // split is that a caller can tell a refusal by Windows from this crate's own
    // retry giving up; asserting only `is_err` would pass just as happily if
    // every failure collapsed back into one shape.
    assert!(
        matches!(outcome, Err(FullPathError::Win32(_))),
        "an empty path names nothing, and Windows says so rather than this crate"
    );
}

#[test]
fn the_two_failures_are_distinguishable() {
    // The defect this replaced: the unstable case returned a synthesized
    // ERROR_INSUFFICIENT_BUFFER, a code Win32 also returns on its own, so these
    // two values were indistinguishable to a caller matching on the error.
    let refused = FullPathError::from(Win32Error::from_code(ERROR_INSUFFICIENT_BUFFER));
    let gave_up = FullPathError::Unstable {
        attempts: super::MAX_ATTEMPTS,
    };

    assert!(matches!(refused, FullPathError::Win32(_)));
    assert!(matches!(gave_up, FullPathError::Unstable { .. }));

    // And they do not read the same either, so a log distinguishes them too.
    assert_ne!(refused.to_string(), gave_up.to_string());
}

#[test]
fn an_unstable_failure_reports_the_attempts_it_made() {
    let error = FullPathError::Unstable {
        attempts: super::MAX_ATTEMPTS,
    };

    assert!(
        error.to_string().contains(&super::MAX_ATTEMPTS.to_string()),
        "the attempt count is the one diagnostic this variant carries: {error}"
    );
}

#[test]
fn only_a_win32_failure_carries_a_source() {
    use std::error::Error;

    // `Unstable` is this crate's own conclusion rather than something Windows
    // reported, so it has no underlying error to chain to.
    let refused = FullPathError::from(Win32Error::from_code(ERROR_INSUFFICIENT_BUFFER));
    let gave_up = FullPathError::Unstable {
        attempts: super::MAX_ATTEMPTS,
    };

    assert!(refused.source().is_some());
    assert!(gave_up.source().is_none());
}

#[test]
fn the_request_reports_the_path_it_was_built_from() {
    let request = ResolveFullPath::new(Wtf16String::from(r"C:\Windows"));

    assert_eq!(request.path().to_string_lossy(), r"C:\Windows");
}

#[test]
fn a_request_is_cloneable_and_resolves_the_same_way() {
    // Unlike the handle-taking entries, this one owns no handle, so an
    // infallible Clone is honest here.
    let request = ResolveFullPath::new(Wtf16String::from(r"C:\Windows\.\System32"));
    let clone = request.clone();

    assert_eq!(
        clone
            .perform()
            .expect("the clone resolves")
            .to_string_lossy(),
        request
            .perform()
            .expect("the original resolves")
            .to_string_lossy()
    );
}

#[test]
fn a_resolution_performs_the_same_way_on_another_thread() {
    const fn assert_send<T: Send>() {}
    const fn assert_sync<T: Sync>() {}

    assert_send::<ResolveFullPath>();
    assert_sync::<ResolveFullPath>();

    let request = ResolveFullPath::new(Wtf16String::from(r"C:\Windows\.\System32"));

    let resolved = std::thread::spawn(move || {
        request
            .perform()
            .expect("resolve on a worker")
            .to_string_lossy()
    })
    .join()
    .expect("the worker did not panic");

    assert_eq!(resolved, r"C:\Windows\System32");
}

/// The current directory this process is running in, as `GetFullPathNameW`
/// would use it.
///
/// Read through the same API rather than `std::env::current_dir`, because the
/// two can disagree: `std` normalizes, and what these tests need is exactly the
/// string the call under test roots against.
fn current_directory() -> String {
    // A lone `.` is rooted at the current directory and then collapses to it.
    resolve(".")
}

#[test]
fn a_relative_path_is_rooted_at_the_process_current_directory() {
    // The rooting half of the documented contract. Asserted as a RELATION to
    // the current directory rather than against a literal, so it holds on any
    // machine and in any working directory.
    let expected = format!(r"{}\rel.txt", current_directory().trim_end_matches('\\'));
    assert_eq!(resolve("rel.txt"), expected);
}

#[test]
fn a_root_relative_path_takes_the_root_and_not_the_whole_directory() {
    // `\foo` is documented as taking only the ROOT of the current directory,
    // which is what distinguishes it from an ordinary relative path. Calling it
    // "the current drive" was wrong -- under a UNC current directory there is
    // no drive at all -- so this pins the property the doc actually claims.
    let cwd = current_directory();
    let resolved = resolve(r"\foo");

    // Computed, not inferred. An earlier version of this test derived the root
    // by trimming the result it was checking, and guarded its one distinguishing
    // assertion behind a length comparison -- so with the current directory at a
    // drive root, where a root-relative path and a relative one coincide, the
    // guard was false and the test passed having asserted almost nothing. A
    // check whose strength depends on where it runs is the vacuous pass this
    // repository keeps paying for.
    let root = root_of(&cwd);
    assert_eq!(
        resolved,
        format!(r"{root}foo"),
        "a root-relative path is rooted at the root of the current directory \
         ({cwd}, root {root}), and carries none of its subtree"
    );
}

/// The root of an absolute Windows path, including its trailing separator.
///
/// C:\a\b gives C:\, and \\server\share\a gives \\server\share\ -- which
/// is why this returns a *root* rather than a drive: under a UNC current
/// directory there is no drive letter to return.
fn root_of(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\") {
        // server\share, then everything after it.
        let mut parts = rest.splitn(3, '\\');
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        return format!(r"\\{server}\{share}\");
    }
    let (drive, _) = path.split_at(2);
    format!(r"{drive}\")
}

#[test]
fn a_legacy_device_name_short_circuits_rooting() {
    // The exception to "roots a path that is not fully qualified", and the one
    // a caller passing an untrusted name has to know about: these do not become
    // files under the current directory.
    for name in [
        "CON", "NUL", "PRN", "AUX", "CONIN$", "CONOUT$", "COM1", "LPT9",
    ] {
        let resolved = resolve(name);
        assert!(
            resolved.starts_with(r"\\.\"),
            "{name} names a device, so it must not be rooted: {resolved}"
        );
    }
}

#[test]
fn the_device_form_accepts_trailing_colons_dots_spaces_and_any_casing() {
    // Every spelling the module doc claims reaches a device. A filter written
    // from a narrower reading of the rule would let these through.
    for spelling in ["CON", "CON:", "CON::", "CON.", "CON ", "con", "cOn:"] {
        let resolved = resolve(spelling);
        assert!(
            resolved.starts_with(r"\\.\"),
            "{spelling:?} is a device spelling: {resolved}"
        );
    }
}

#[test]
fn superscript_digits_are_device_names_too() {
    // The members a hand-written denylist omits, and which this crate's own
    // documentation asserted did not exist until a review measured them. If a
    // future Windows build stops accepting them this test says so, which is the
    // whole reason it is here rather than left as prose.
    for spelling in [
        "COM\u{00b9}",
        "COM\u{00b2}",
        "COM\u{00b3}",
        "LPT\u{00b9}",
        "LPT\u{00b2}",
        "LPT\u{00b3}",
    ] {
        let resolved = resolve(spelling);
        assert!(
            resolved.starts_with(r"\\.\"),
            "{spelling:?} uses a superscript digit and still names a device: {resolved}"
        );
    }
}

#[test]
fn a_device_name_with_anything_around_it_is_rooted_normally() {
    // The other half, and the one that keeps the rule from being read as "any
    // input containing a device name". Without these the test above would pass
    // just as well against an implementation that mapped far too much.
    for spelling in [
        "CON.txt", r"a\CON", r".\CON", "CON:x", "COM0", "COM10", "CONIN",
    ] {
        let resolved = resolve(spelling);
        assert!(
            !resolved.starts_with(r"\\.\"),
            "{spelling:?} is not a bare device name, so it must be rooted: {resolved}"
        );
    }
}

#[test]
fn a_fully_qualified_path_is_unaffected_by_the_current_directory() {
    // The lexical half, stated as the invariance the rooting half lacks: this
    // is what makes "not lexical as a whole" a claim about the OTHER half only.
    // `C:\a` need not exist, which is the same fact the existence test pins.
    assert_eq!(resolve(r"C:\a\..\b"), r"C:\b");
    assert_eq!(resolve("C:/a/b//c"), r"C:\a\b\c");
}
