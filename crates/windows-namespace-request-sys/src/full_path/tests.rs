// Copyright (c) Mike Grier.

//! Tests for the `GetFullPathNameW` entry.
//!
//! The negatives matter more than the positives here: this call does not verify
//! what it produces, and a suite that only ever resolved existing paths would
//! leave a reader believing it does.

use windows_sys::Win32::Foundation::{
    ERROR_ENVVAR_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
};
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
    //
    // The components are process-specific and their absence is asserted first.
    // A hard-coded literal is only missing until some host happens to have it,
    // and this test would then be demonstrating that an EXISTING path resolves
    // -- which every other test here already covers.
    let missing = std::env::temp_dir().join(format!("wnrs-{}-absent", std::process::id()));
    let missing = missing.to_str().expect("the temp path is UTF-8");
    assert!(
        !std::path::Path::new(missing).exists(),
        "precondition: {missing} must not exist"
    );

    let doubled = format!(r"{missing}\also-absent\..\leaf.txt");
    assert_eq!(resolve(&doubled), format!(r"{missing}\leaf.txt"));
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
    // Stated rather than assumed: the only caller passes a resolved absolute
    // path, which is always at least `C:\`, but `split_at` would panic on an
    // index rather than say why.
    assert!(
        path.len() >= 2 && path.is_char_boundary(2),
        "root_of expects an absolute path, got {path:?}"
    );
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

#[test]
fn a_drive_relative_path_carries_its_component_and_the_current_drive_uses_the_process_directory() {
    // **The name says what the two assertions reach, and an earlier one did
    // not.** This was
    // `a_drive_relative_path_is_rooted_at_that_drive_and_not_the_process_directory`,
    // which claims more than anything here shows, in two separate ways. The
    // other-drive assertion is `ends_with("\\foo")`, which accepts ANY base
    // including the process directory -- so it cannot say "not the process
    // directory". And "rooted at that drive" is not even true in general: an
    // accepted entry is used verbatim and may name a directory on a different
    // drive entirely, which is the sibling test's whole point.
    //
    // The body already said it only bounds the arm. The name did not, and the
    // name is what a reader takes away -- the same defect as the test called
    // `..._is_neither_consulted_nor_rewritten` before it was renamed.
    //
    // The third rooting form, with two arms, which is the part that gets
    // missed:
    //
    //   * For a drive OTHER than the current one, Windows reads the hidden
    //     `=X:` entry recorded for it.
    //   * For the CURRENT drive the entry makes no difference to the result and
    //     the process current directory wins. Measured: setting `=Q:` while the
    //     process is on `Q:` changes nothing.
    //
    // **This test does not mutate `=X:`, but the call it exercises may.**
    // Measured: resolving `X:foo` for a non-current drive checks that drive's
    // entry and WRITES it to `X:\` when the entry is absent or rejected. An
    // accepted entry is left alone, and the current-drive form writes nothing
    // -- so this is not "every resolution", but it does mean an ordinary host
    // with no entry has one written merely by running this test. That is
    // a property of the call, documented in the module doc; it is noted here so
    // the next reader does not take "reads process state" at face value, as
    // four revisions of that doc did.
    //
    // **This test only BOUNDS the other-drive arm**, because it does not control
    // the entry: with no `=X:` set, an implementation that always used the
    // drive root would satisfy everything here. The arm is pinned properly by
    // `a_drive_relative_path_uses_that_drives_entry_verbatim_and_rewrites_a_bad_one`,
    // which sets the entry and draws its letter from a disjoint pair, so the
    // two cannot race.
    let cwd = current_directory();
    let cwd_drive = cwd.chars().next().filter(char::is_ascii_alphabetic);

    // The other-drive arm needs no drive letter from the current directory --
    // under a UNC current directory every letter is "other" -- so it runs
    // unconditionally and this test never degenerates to a silent skip.
    let other = probe_drive_from(probe_drives::ROOTED_AT_THAT_DRIVE, None);

    // This test controls no entry, but the CALL does: resolving for a
    // non-current drive writes `=X:` whenever the recorded entry is absent or
    // rejected, and on most hosts a letter chosen for being unused has no entry
    // at all. So merely observing the arm mutates process-global state, and
    // this was the one mutating case here without a guard -- the borrow is
    // needed exactly because the mutation is not the test's own doing.
    let _restore = BorrowedDriveEntry::take(other);
    let resolved = resolve(&format!("{other}:foo"));

    // Only what is invariant without controlling the entry. An earlier version
    // required the result to start with `X:\`, which the verbatim rule breaks;
    // its replacement compared against the current directory, which `other`
    // differs from by construction, so it could fire only if the entry happened
    // to equal the process directory exactly -- the same vacuity, respelled.
    // What survives every entry value is that the component is carried through.
    assert!(
        resolved.ends_with(r"\foo"),
        "the component is carried through whatever the entry holds: {resolved}"
    );

    // The current-drive arm, where the process directory wins over any `=X:`.
    // Only expressible when the current directory has a drive letter at all.
    if let Some(drive) = cwd_drive {
        assert_eq!(
            resolve(&format!("{drive}:foo")),
            format!(r"{}\foo", cwd.trim_end_matches('\\')),
            "on the current drive, the per-drive entry makes no difference to \
             the result and the process directory is used"
        );
    }
}

#[test]
fn the_current_drives_entry_does_not_affect_resolution_and_is_not_rewritten() {
    // The module doc states both halves of the current-drive arm as fact. Until
    // now nothing pinned either, and the assertion just above -- which looks
    // like it does -- cannot: it resolves `X:foo` WITHOUT controlling the entry
    // and compares against the process directory, and Windows keeps the current
    // drive's entry equal to that directory. So it reads the same either way.
    // Vacuous in precisely the way this crate keeps rediscovering, and the
    // reason the two arms need opposite fixtures: the sibling tests must AVOID
    // the current drive, and this one must be on it.
    //
    // **The name says what is observable, and an earlier one did not.** This
    // was `..._is_neither_consulted_nor_rewritten`, which claims the entry is
    // not READ -- and installing a value and watching the outcome cannot
    // separate "not read" from "read and ignored". That is the same overreach
    // this branch removed from the probe's "without consulting a device", and
    // the test correcting it committed it in its own name. What the two
    // assertions below reach is the pair of observable effects: the entry makes
    // no difference to the result, and it is not written back.
    let cwd = current_directory();
    let Some(drive) = cwd.chars().next().filter(char::is_ascii_alphabetic) else {
        // A UNC current directory has no drive letter, so there is no
        // current-drive arm to exercise. Not a skip of something testable.
        return;
    };
    let process_directory = format!(r"{}\foo", cwd.trim_end_matches('\\'));

    let probe_dir = probe_directory("current-drive");
    let probe = probe_dir.path.to_str().expect("the probe path is UTF-8");
    let _restore = BorrowedDriveEntry::take(drive);

    // The anti-vacuity check, made permanent rather than performed once by
    // hand: unless the two arms would give DIFFERENT answers, every assertion
    // below passes without distinguishing them, which is the failure this test
    // was written to correct.
    assert_ne!(
        process_directory,
        format!(r"{probe}\foo"),
        "precondition: the entry must name somewhere other than the process \
         directory, or honouring it and ignoring it look identical"
    );

    // No difference to the result. The entry is one the OTHER arm would honour
    // verbatim -- an existing directory in canonical form -- and it names
    // somewhere the process directory cannot be, because this test just created
    // it under a process-unique name. If the entry were HONOURED, the result
    // would be under `probe`.
    set_drive_entry(drive, Some(probe));
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        process_directory,
        "the current drive's entry was set to {probe}, an entry the non-current \
         arm honours verbatim, and the process directory won anyway"
    );

    // Not rewritten, which needs a REJECTED entry to be visible: an accepted one
    // is left alone on both arms, so leaving it alone shows nothing. A child of
    // the probe directory cannot exist, and on the non-current arm that is
    // replaced by the drive root.
    let missing = probe_dir.path.join("no-such-child");
    let missing = missing.to_str().expect("the probe path is UTF-8");
    set_drive_entry(drive, Some(missing));
    let _ = resolve(&format!("{drive}:foo"));
    assert_eq!(
        drive_entry(drive).map(|v| v.to_string_lossy()).as_deref(),
        Some(missing),
        "an entry the non-current arm would have replaced with {drive}:\\ is \
         left untouched on the current drive"
    );
}

#[test]
fn trailing_dot_and_space_trimming_differs_between_final_and_intermediate_components() {
    // The module doc says the rewrite trims trailing dots and spaces. Until now
    // that was only exercised through a final `.` component (which is the
    // separate `.`-collapsing rule) and through device spellings (which take
    // the short-circuit and never reach the ordinary path). Neither pins this.
    //
    // **An earlier version of this test generalised the rule, and the
    // generalisation is false.** It carried one intermediate case, `C:\a.\b` ->
    // `C:\a\b`, under the comment "trimming applies per component, not only at
    // the end of the path". Measured, a component's position decides what it
    // loses:
    //
    //   final:        C:\name...   -> C:\name     (a RUN of dots goes)
    //                 C:\name      -> C:\name     (trailing spaces go)
    //   intermediate: C:\a.\b      -> C:\a\b      (ONE trailing dot goes)
    //                 C:\a.b.\c    -> C:\a.b\c    (inner dots are not special)
    //                 C:\a...\b    -> unchanged   (a run does NOT go)
    //                 C:\a \b      -> unchanged   (a space does NOT go)
    //                 C:\a. \b     -> unchanged
    //
    // **The preservation cases are the load-bearing half**, and their absence
    // is what let the wrong generalisation stand: with only the transforming
    // inputs, an implementation trimming every component's trailing dots and
    // spaces satisfies the entire test while being wrong about three of the
    // five intermediate spellings. A reviewer demonstrated exactly that against
    // the previous seven assertions.
    //
    // Every case measured before being written down -- which was also claimed
    // last time, and was true of the cases present. What was not measured was
    // the sentence generalising them.
    for (input, expected) in [
        // The final component loses any run of trailing dots and spaces.
        (r"C:\name.", r"C:\name"),
        (r"C:\name ", r"C:\name"),
        (r"C:\name...", r"C:\name"),
        (r"C:\name   ", r"C:\name"),
        (r"C:\name. ", r"C:\name"),
        // An extension is not special: the trailing dot goes, the rest stays.
        (r"C:\name.txt.", r"C:\name.txt"),
        // An intermediate component loses a single trailing dot ...
        (r"C:\a.\b", r"C:\a\b"),
        (r"C:\a.b.\c", r"C:\a.b\c"),
        // ... and nothing else. These are the cases that fail the broad rule.
        (r"C:\a...\b", r"C:\a...\b"),
        (r"C:\a \b", r"C:\a \b"),
        (r"C:\a. \b", r"C:\a. \b"),
        (r"C:\a b \c", r"C:\a b \c"),
    ] {
        assert_eq!(resolve(input), expected, "trimming {input:?}");
    }
}

#[test]
fn a_name_containing_a_device_word_is_rooted_under_the_current_directory() {
    // Strengthens the device-negative control. Asserting only "not `\\.\`" is
    // too weak: an implementation that returned every relative input unchanged
    // would satisfy it while rooting nothing. These assert the full resolved
    // path, so the rooting guarantee is actually covered.
    //
    let base = current_directory();
    let base = base.trim_end_matches('\\');
    // `CON:x` is here because it was NOT, and the negative test alone cannot
    // carry it: that test asserts only `!starts_with("\\\\.\\")`, which the
    // unrooted literal `CON:x` satisfies. So the one spelling whose rooting is
    // least obvious -- a device word followed by a colon, where `CON:` itself
    // IS a device -- was the one nothing pinned. Measured: `<cwd>\CON:x`, with
    // the colon carried through untouched.
    for name in ["CON.txt", "CONIN", "COM0", "COM10", r"a\CON", "CON:x"] {
        assert_eq!(
            resolve(name),
            format!(r"{base}\{name}"),
            "{name:?} is not a bare device name, so it roots under the current \
             directory rather than merely avoiding the device namespace"
        );
    }

    // `.\CON` is asserted separately rather than excluded from the loop, which
    // is what an earlier version did on the grounds that the `.` collapses and
    // the expected form would need a special case. It needs one, so it gets
    // one: with the case left to the device-NEGATIVE test alone, the only claim
    // made about `.\CON` was that it does not start with `\\.\` -- and an
    // implementation returning it unchanged satisfies that, which is precisely
    // the too-weak assertion this test exists to strengthen.
    assert_eq!(
        resolve(r".\CON"),
        format!(r"{base}\CON"),
        "the `.` collapses and the result roots under the current directory, so \
         a leading `.\\` is enough to take the name out of the device \
         short-circuit without taking it out of ordinary rooting"
    );

    // The root-relative form, which the module doc states and nothing pinned.
    // Its expectation is the current directory's ROOT rather than `base`, so it
    // cannot join the loop above.
    let root = root_of(&current_directory());
    assert_eq!(
        resolve(r"\CON"),
        format!("{root}CON"),
        "a leading separator roots the device word at the current directory's \
         root instead of reaching the device"
    );
}

#[test]
fn nul_is_the_one_device_word_a_path_around_it_does_not_save() {
    // **The exception to the test above, and it was found by trying to write
    // the general rule.** A review asked for the `\CON` case on the grounds
    // that the doc states it; the doc stated it of the whole device SET, having
    // been written from `CON` alone. Measured across all eight accepted names,
    // seven root normally once anything precedes them and `NUL` does not --
    // `NUL` short-circuits as the final component of any path at all.
    //
    // Had the requested assertion been written as the general rule it was
    // phrased as ("a root-relative device word roots at the root"), it would
    // have pinned a false claim, which is the failure this branch exists to
    // stop rather than repeat.
    let base = current_directory();
    let base = base.trim_end_matches('\\');
    let root = root_of(&current_directory());

    // The seven that behave as the doc says, in the form that separates them.
    for name in ["CON", "PRN", "AUX", "CONIN$", "CONOUT$", "COM1", "LPT1"] {
        assert_eq!(
            resolve(&format!(r".\{name}")),
            format!(r"{base}\{name}"),
            "{name:?} stops being a device once a path precedes it"
        );
        assert_eq!(
            resolve(&format!(r"C:\{name}")),
            format!(r"C:\{name}"),
            "{name:?} is an ordinary component of a fully-qualified path"
        );
    }

    // And `NUL`, which does not -- INCLUDING from a fully-qualified path, so a
    // rooted result is not by itself evidence that a path names a file.
    for input in [r"\NUL", r".\NUL", r"a\NUL", r"C:\NUL"] {
        assert_eq!(
            resolve(input),
            r"\\.\NUL",
            "{input:?} still reaches the device: NUL is not saved by a path in \
             front of it, where every other device word is"
        );
    }

    // A suffix is what takes it out, which is the boundary of the exception --
    // and the doc names both spellings, so both are asserted. The root-relative
    // form alone would leave the bare ones uncovered, which is how `CON:x` came
    // to be pinned by a predicate the wrong answer satisfied.
    assert_eq!(
        resolve(r"\NUL.txt"),
        format!("{root}NUL.txt"),
        "an extension takes even NUL out of the device namespace"
    );
    for (input, expected) in [("NUL.txt", "NUL.txt"), ("NUL:x", "NUL:x")] {
        assert_eq!(
            resolve(input),
            format!(r"{base}\{expected}"),
            "{input:?} is a suffixed name, so it roots under the current \
             directory like any other"
        );
    }

    // And the boundary is a suffix, not merely "more characters": a trailing
    // colon or space still reaches the device, exactly as for `CON`. Without
    // these the rule above would read as "anything after NUL saves it".
    for input in ["NUL::", "NUL "] {
        assert_eq!(
            resolve(input),
            r"\\.\NUL",
            "{input:?} is still the device: trailing colons and spaces are part \
             of the device form, not a suffix that escapes it"
        );
    }
}

/// A directory that exists, is in canonical `X:\...` form, and is neither a
/// drive root nor the process current directory.
///
/// **The form is enforced, not assumed, and that distinction has now cost two
/// rounds.** An earlier version derived the probe from `current_directory()`,
/// which yields `C:` at a drive root -- drive-relative, not a directory -- and
/// both tests failed there. Its replacement used `std::env::temp_dir()`, which
/// is `%TMP%`/`%TEMP%` verbatim and carries no guarantee of a drive letter: with
/// temp redirected to a share, the probe is a UNC path, which
/// `GetFullPathNameW` rejects as an entry *on shape* -- the very rule the
/// caller is trying to pin. Folder redirection makes that an ordinary
/// configuration, not a contrived one.
///
/// So the temp directory is used only when it is drive-rooted, and otherwise
/// the fallback is `%SystemRoot%`, which is guaranteed to exist, to be
/// canonical, and not to be a drive root. Nothing is created in the fallback
/// case, so `created` records whether there is anything to remove.
///
/// Removal is a [`Drop`], matching this crate's own `Fixture` in
/// [`crate::handle`]'s tests. An earlier version cleaned up with a statement at
/// the end of each test and argued that a guard writing during unwinding could
/// panic and abort -- which is true of the `=X:` entry restore beside it, and
/// not of removing a directory. Conflating the two left a directory behind
/// after every failing assertion.
struct ProbeDir {
    path: std::path::PathBuf,
    created: bool,
}

impl ProbeDir {
    /// The drive letter this probe lives on, if it has one.
    ///
    /// The caller needs it to pick a probe drive that is *not* this one:
    /// resolving `W:foo` against an entry naming a directory that is itself on
    /// `W:` cannot demonstrate that an accepted entry is used verbatim across
    /// drives, which is the property being pinned.
    fn drive(&self) -> Option<char> {
        self.path
            .as_os_str()
            .to_string_lossy()
            .chars()
            .next()
            .filter(char::is_ascii_alphabetic)
    }
}

impl Drop for ProbeDir {
    fn drop(&mut self) {
        if self.created {
            let _ = std::fs::remove_dir(&self.path);
        }
    }
}

fn probe_directory(tag: &str) -> ProbeDir {
    // The full shape an accepted `=X:` entry must have, not just its first
    // three characters: rooted at `X:\`, with no `.` or `..` component and no
    // forward slash. `a_rejected_drive_entry_is_replaced_by_the_drive_root`
    // shows each of those spellings is rejected while naming the same existing
    // directory, so a base carrying one would turn that test's CONTROL
    // assertion -- "the same directory in canonical form is accepted" -- into a
    // rejection, and it would fail for a reason unrelated to what it pins.
    //
    // **A review read that as reachable through a non-canonical `%TMP%`. It is
    // not, and the measurement is here so the next reader need not repeat it.**
    // `std::env::temp_dir` goes through `GetTempPath2W`, which normalises what
    // it finds: `C:/Users/.../Temp`, `...\Temp\.`, `...\Temp\..\Temp`,
    // `...\Temp\\` and even the drive-relative `C:Users\...` all came back as
    // `C:\Users\...\Temp\`. The one spelling passed through verbatim is
    // `\\?\C:\...`, which is not drive-rooted and so takes the fallback below.
    //
    // The check is widened anyway. It costs nothing, it covers the fallback
    // base too, and it is the difference between a precondition that is
    // enforced and one that is argued -- which is the distinction this whole
    // change exists to hold.
    let canonical_drive_rooted = |p: &std::path::Path| {
        let s = p.as_os_str().to_string_lossy().into_owned();
        let mut chars = s.chars();
        matches!(
            (chars.next(), chars.next(), chars.next()),
            (Some(d), Some(':'), Some('\\')) if d.is_ascii_alphabetic()
        ) && !s.contains('/')
            && !s.split('\\').any(|c| c == "." || c == "..")
    };

    let temp = std::env::temp_dir();
    if canonical_drive_rooted(&temp) {
        let path = temp.join(format!("wnrs-{}-{tag}", std::process::id()));

        // `create_dir`, not `create_dir_all`, and the difference is ownership
        // rather than parents. `create_dir_all` SUCCEEDS on a directory that
        // already exists, so setting `created` after it recorded a claim this
        // fixture had not established -- and `Drop` then removes the path on
        // the strength of that claim. The name is `%TEMP%\wnrs-<pid>-<tag>`,
        // which an interrupted earlier run leaves behind and which Windows can
        // hand back to a later process when it reuses the PID. The blast radius
        // is small, because `remove_dir` refuses a non-empty directory -- but
        // "small" is not the point. Deleting something on an ownership claim
        // nothing checked is the same defect as asserting a mechanism nothing
        // measured, and this file exists to stop doing that.
        //
        // An existing directory is still perfectly usable as a probe; it is
        // just not ours to remove.
        let created = match std::fs::create_dir(&path) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => panic!("create the probe directory {}: {e}", path.display()),
        };

        return ProbeDir { path, created };
    }

    // Not creating anything here, so no write permission is needed on a host
    // whose temp directory is redirected off a drive letter.
    //
    // **It must also differ from the process current directory**, which the
    // temp branch gets for free -- it creates a uniquely named child -- and this
    // branch does not. Cargo launched from `%SystemRoot%` on a host with a UNC
    // temp directory would otherwise hand back the current directory itself,
    // and a probe indistinguishable from the current directory cannot separate
    // "the entry was honoured" from "the entry was ignored". `System32` is the
    // second candidate for the same reason `probe_drive_from` takes a list:
    // one value that is usually right is not a guarantee.
    let system_root = std::path::PathBuf::from(
        std::env::var("SystemRoot").expect("SystemRoot is set on Windows"),
    );
    let cwd = current_directory();
    let cwd = cwd.trim_end_matches('\\');
    let distinct = |p: &std::path::Path| {
        !p.as_os_str()
            .to_string_lossy()
            .trim_end_matches('\\')
            .eq_ignore_ascii_case(cwd)
    };

    let path = [system_root.clone(), system_root.join("System32")]
        .into_iter()
        .find(|p| canonical_drive_rooted(p) && distinct(p))
        .unwrap_or_else(|| {
            panic!(
                "no fallback probe directory is both canonical and distinct \
                 from the current directory {cwd}"
            )
        });

    ProbeDir {
        path,
        created: false,
    }
}
/// The candidate drive letters, one list per test that mutates a `=X:` entry.
///
/// **Centralised so the properties these tests depend on are CHECKED rather
/// than restated.** Both were previously prose -- a doc comment saying the
/// lists are disjoint, and an archived note enumerating them -- and prose
/// drifted: the archive named five lists after the sixth had been added, so a
/// reader picking letters for a seventh would have consulted an inventory
/// missing three of the eighteen letters already in use. Nothing checked
/// either claim, because nothing could: the lists were literals at six call
/// sites with no table to read.
///
/// [`the_probe_drive_candidate_lists_are_disjoint_and_large_enough`] now reads
/// this table, so adding a list that collides -- or one too short for
/// [`probe_drive_from`]'s guarantee -- fails a test instead of a review.
mod probe_drives {
    pub const ROOTED_AT_THAT_DRIVE: &[char] = &['X', 'Y', 'P'];
    pub const VERBATIM_ENTRY: &[char] = &['W', 'U', 'N'];
    pub const REJECTED_ENTRY: &[char] = &['V', 'T', 'M'];
    pub const LONG_ENTRY: &[char] = &['R', 'S', 'K'];
    pub const BORROW_GUARD: &[char] = &['G', 'H', 'J'];
    pub const EMPTY_VS_ABSENT: &[char] = &['E', 'F', 'B'];

    /// Every list above. A new list that is not added here is not covered by
    /// the disjointness test, so keep them together.
    pub const ALL: &[(&str, &[char])] = &[
        ("ROOTED_AT_THAT_DRIVE", ROOTED_AT_THAT_DRIVE),
        ("VERBATIM_ENTRY", VERBATIM_ENTRY),
        ("REJECTED_ENTRY", REJECTED_ENTRY),
        ("LONG_ENTRY", LONG_ENTRY),
        ("BORROW_GUARD", BORROW_GUARD),
        ("EMPTY_VS_ABSENT", EMPTY_VS_ABSENT),
    ];
}

#[test]
fn the_probe_drive_candidate_lists_are_disjoint_and_large_enough() {
    for (name, list) in probe_drives::ALL {
        // `probe_drive_from` excludes at most two letters -- the current drive
        // and the probe directory's drive -- so three candidates guarantee a
        // survivor. This is the premise of the panic in that function, checked
        // here rather than left to the caller as the doc comment used to.
        assert!(
            list.len() >= 3,
            "{name} has {} candidates, and at most two can be excluded, so \
             fewer than three cannot guarantee a survivor",
            list.len()
        );

        let mut seen = list.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), list.len(), "{name} repeats a letter");
    }

    for (a_name, a) in probe_drives::ALL {
        for (b_name, b) in probe_drives::ALL {
            if a_name == b_name {
                continue;
            }
            let shared: Vec<char> = a
                .iter()
                .copied()
                .filter(|c| b.iter().any(|d| d.eq_ignore_ascii_case(c)))
                .collect();
            assert!(
                shared.is_empty(),
                "{a_name} and {b_name} share {shared:?}, so the two tests can \
                 select the same drive and race under libtest's \
                 thread-per-test model"
            );
        }
    }
}

/// A drive letter to probe with, drawn from `candidates` and guaranteed to be
/// neither the current drive nor `avoid`.
///
/// **Every candidate is checked, which an earlier version did not do.** It took
/// a preferred letter and a fallback, tested only the preferred one, and
/// returned the fallback unvalidated -- so when the preferred letter was
/// excluded the caller could still be handed the current drive. Measured: with
/// the process on `U:` and `%TEMP%` on a `subst`-ed `W:`, the verbatim test
/// selected `U` and then asserted the *other-drive* contract while exercising
/// the *current-drive* arm, which is the one case where the entry makes no
/// difference to the result.
/// It failed, but the mode is worse than a failure: the helper's own doc
/// promised a guarantee it never enforced.
///
/// Three candidates against at most two exclusions, so one always survives. The
/// panic remains because that argument is about the caller's list, which this
/// function cannot see -- but the argument is no longer only an argument:
/// [`the_probe_drive_candidate_lists_are_disjoint_and_large_enough`] checks it
/// against every list in [`probe_drives`].
///
/// Callers pass disjoint lists, so no two tests can select the same letter and
/// race under libtest's thread-per-test model.
fn probe_drive_from(candidates: &[char], avoid: Option<char>) -> char {
    let cwd = current_directory();
    // A UNC current directory has no drive letter, so nothing collides there.
    let current = cwd.chars().next().filter(char::is_ascii_alphabetic);
    let taken = |c: char| {
        current.is_some_and(|d| d.eq_ignore_ascii_case(&c))
            || avoid.is_some_and(|d| d.eq_ignore_ascii_case(&c))
    };

    *candidates.iter().find(|&&c| !taken(c)).unwrap_or_else(|| {
        panic!(
            "every candidate of {candidates:?} is excluded by the current \
                 drive ({current:?}) or the probe drive ({avoid:?})"
        )
    })
}
/// Reads one of the hidden `=X:` per-drive current-directory entries.
///
/// Through Win32 rather than `std::env`, which rejects a key containing `=`
/// outright and so cannot address these at all.
fn drive_entry(drive: char) -> Option<Wtf16String> {
    let name = Wtf16String::from(format!("={drive}:").as_str());
    // Start small and grow to whatever Windows asks for. The API's two return
    // conventions differ: on success it reports the units written EXCLUDING the
    // terminator, and on an undersized buffer it reports the capacity REQUIRED
    // INCLUDING it. Treating the second as the first indexes past the buffer and
    // panics -- while trying to preserve a legitimate long entry, so the failure
    // would land before the test could restore the process state it borrowed.
    let mut buffer = vec![0u16; 256];
    loop {
        // Zero is TWO different answers, and the last error is the only thing
        // that separates them -- so it is cleared first, because the value left
        // by some earlier call would otherwise be read as this one's.
        //
        // SAFETY: no preconditions.
        unsafe { windows_sys::Win32::Foundation::SetLastError(ERROR_SUCCESS) };

        // SAFETY: the name is NUL-terminated and the buffer is writable for
        // the length passed.
        let written = unsafe {
            windows_sys::Win32::System::Environment::GetEnvironmentVariableW(
                name.as_terminated_ptr(),
                buffer.as_mut_ptr(),
                u32::try_from(buffer.len()).unwrap_or(u32::MAX),
            )
        };
        let written = written as usize;
        if written == 0 {
            // **An earlier version of this comment claimed, as measured, that
            // an empty value and an absent name are the same state and cannot
            // be told apart. That was wrong, and wrong in this crate's
            // signature way: the measurement behind it never cleared the last
            // error, so it could only ever have seen whatever was already
            // there.** Cleared first and re-measured, the two are distinct, for
            // an ordinary name and an `=X:` name alike:
            //
            //   set to ""  -> returns 0, last error ERROR_SUCCESS
            //   deleted    -> returns 0, last error ERROR_ENVVAR_NOT_FOUND
            //
            // The difference is not academic here. Collapsing both to `None`
            // makes the restoration in `BorrowedDriveEntry` DELETE an inherited
            // empty entry rather than put it back -- losing exactly the process
            // state the guard exists to preserve.
            //
            // SAFETY: no preconditions.
            let last = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            return match last {
                ERROR_SUCCESS => Some(Wtf16String::from_units(&[])),
                ERROR_ENVVAR_NOT_FOUND => None,
                // Anything else is neither answer, and folding it into "absent"
                // would make the guard delete an entry over a transient error.
                other => panic!("reading ={drive}: failed with error {other}"),
            };
        }
        if written < buffer.len() {
            // Kept as WTF-16 units rather than going through String: a lossy
            // conversion would replace an unpaired surrogate, so restoring the
            // entry afterwards would write back something the process did not
            // start with.
            return Some(Wtf16String::from_units(&buffer[..written]));
        }
        buffer = vec![0u16; written];
    }
}
/// Sets or clears one of the hidden `=X:` entries.
fn set_drive_entry(drive: char, value: Option<&str>) {
    set_drive_entry_units(drive, value.map(Wtf16String::from).as_ref());
}

/// [`set_drive_entry`], taking the exact units a [`drive_entry`] read returned.
///
/// Restoration goes through this rather than through `&str`, so an entry
/// containing an unpaired surrogate is put back byte for byte.
fn set_drive_entry_units(drive: char, value: Option<&Wtf16String>) {
    assert!(
        try_set_drive_entry_units(drive, value),
        "set ={drive}: entry"
    );
}

/// [`set_drive_entry_units`] without the assertion, reporting success instead.
///
/// Separate because the restoration in [`BorrowedDriveEntry`] runs during
/// unwinding, where a panic would abort the process and destroy the report of
/// the failure that started the unwind.
fn try_set_drive_entry_units(drive: char, value: Option<&Wtf16String>) -> bool {
    let name = Wtf16String::from(format!("={drive}:").as_str());
    let value_ptr = value
        .as_ref()
        .map_or(core::ptr::null(), |v| v.as_terminated_ptr());
    // SAFETY: both pointers are NUL-terminated; a null value clears the entry.
    let ok = unsafe {
        windows_sys::Win32::System::Environment::SetEnvironmentVariableW(
            name.as_terminated_ptr(),
            value_ptr,
        )
    };
    ok != 0
}

/// Borrows one drive's `=X:` entry and puts it back when the test ends,
/// **whether or not the test panicked**.
///
/// Restoring on the last line of the test is not enough, and the hazard is not
/// theoretical: `=X:` is process-global, `cargo test` runs tests as threads in
/// ONE process, and every assertion between the save and the restore is a place
/// the entry can be abandoned. What a sibling test would then inherit is not
/// merely a stale value but one no host would produce -- a path to a directory
/// that no longer exists once the probe directory is removed, or the 1200-unit
/// value that `a_long_drive_entry_round_trips_through_the_reader` installs on
/// purpose. The reader above already names this ("the entry is then never
/// restored") without defending against it; this is the defence.
///
/// A failed restore is dropped rather than asserted, for the reason given on
/// [`try_set_drive_entry_units`].
struct BorrowedDriveEntry {
    drive: char,
    saved: Option<Wtf16String>,
}

impl BorrowedDriveEntry {
    fn take(drive: char) -> Self {
        Self {
            drive,
            saved: drive_entry(drive),
        }
    }
}

impl Drop for BorrowedDriveEntry {
    fn drop(&mut self) {
        let restored = try_set_drive_entry_units(self.drive, self.saved.as_ref());

        // Silence is bought only where it buys something. While unwinding, a
        // panic here aborts the process and destroys the report of the failure
        // that started the unwind, so a failed restore is worth less than the
        // diagnosis it would replace. On the ordinary path there is no such
        // trade: staying quiet would let the suite carry on with corrupted
        // process-global state and fail somewhere unrelated, which is the
        // hardest kind of failure to read.
        assert!(
            restored || std::thread::panicking(),
            "restoring ={}: failed, leaving process-global state corrupted for \
             every test that follows",
            self.drive
        );
    }
}

#[test]
fn a_drive_relative_path_uses_that_drives_entry_verbatim_and_rewrites_a_bad_one() {
    // The arm the sibling test can only BOUND. Without controlling the entry,
    // an implementation that always used the drive root would satisfy every
    // assertion there, because a host with no `=X:` entry cannot tell the two
    // rules apart.
    //
    // **Controlling the entry is not the hazard it looks like**, and that is
    // what unblocked this test. The objection was that `=X:` is process-global
    // while these tests share a process. But the call under test writes that
    // entry itself whenever it is absent or rejected, so this state is already
    // mutated by the code being exercised. What keeps the tests from
    // interfering is not that -- it is that each takes a drive letter no other
    // one can choose.
    //
    // The probe directory is created FIRST so the letter can avoid its drive as
    // well as the current one: an entry naming a directory on the same drive it
    // is recorded for would be honoured, the test would pass, and the
    // cross-drive property below would go unexercised.
    let probe_dir = probe_directory("verbatim");
    let probe = probe_dir.path.to_str().expect("the probe path is UTF-8");
    let drive = probe_drive_from(probe_drives::VERBATIM_ENTRY, probe_dir.drive());
    let _restore = BorrowedDriveEntry::take(drive);

    assert_ne!(
        Some(drive.to_ascii_uppercase()),
        probe_dir.drive().map(|d| d.to_ascii_uppercase()),
        "the probe directory must be on a different drive, or the assertion \
         below cannot show the entry is honoured ACROSS drives"
    );

    set_drive_entry(drive, Some(probe));
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        format!(r"{probe}\foo"),
        "an entry naming an existing directory is honoured verbatim, even onto \
         a different drive -- so \"that drive's own current directory\" is the \
         convention the entry usually holds, not a guarantee about the result"
    );

    // Verbatim means verbatim, including the join. The module doc records that
    // an accepted entry ending in a separator yields a DOUBLED one, and nothing
    // pinned it -- so the observation could have stopped being true without CI
    // noticing, which is the drift this change exists to close rather than
    // commit again.
    set_drive_entry(drive, Some(&format!(r"{probe}\")));
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        format!(r"{probe}\\foo"),
        "an entry is accepted with a trailing separator and concatenated \
         without normalising the join"
    );

    // An entry that does not name an existing directory is rejected, and the
    // call rewrites it to the drive root rather than leaving it stale.
    //
    // Derived from the probe directory rather than hard-coded: a literal like
    // `C:\no-such-directory-for-this-test` is only missing until some host
    // happens to have it, and the test would then assert the rejected case
    // against an accepted one. A child of a directory this test just created
    // cannot exist unless something else creates it in between.
    let missing = probe_dir.path.join("no-such-child");
    assert!(
        !missing.exists(),
        "precondition: the rejected entry must name nothing: {}",
        missing.display()
    );
    set_drive_entry(
        drive,
        Some(missing.to_str().expect("the probe path is UTF-8")),
    );
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        format!(r"{drive}:\foo"),
        "an entry that names nothing is rejected in favour of the drive root"
    );
    assert_eq!(
        drive_entry(drive).map(|v| v.to_string_lossy()).as_deref(),
        Some(format!(r"{drive}:\").as_str()),
        "and the call REWROTE the entry: this is a query that mutates the \
         process environment block"
    );

    // Absent entirely, the entry is created rather than merely read.
    set_drive_entry(drive, None);
    assert_eq!(drive_entry(drive), None, "precondition: entry cleared");
    let _ = resolve(&format!("{drive}:foo"));
    assert_eq!(
        drive_entry(drive).map(|v| v.to_string_lossy()).as_deref(),
        Some(format!(r"{drive}:\").as_str()),
        "resolving created the entry on a host that had none"
    );
}

#[test]
fn a_rejected_drive_entry_is_replaced_by_the_drive_root() {
    // Acceptance needs BOTH a shape and an existence check, and a draft of the
    // module doc claimed it was "a filesystem query rather than a syntax test"
    // -- having measured only the existence half. Every value below names an
    // existing directory, so anything rejected here is rejected on shape alone.
    //
    // Pinned because the distinction is not guessable and the doc asserts it.
    let probe_dir = probe_directory("shape");
    let accepted = probe_dir.path.to_str().expect("the probe path is UTF-8");
    let drive = probe_drive_from(probe_drives::REJECTED_ENTRY, probe_dir.drive());
    let _restore = BorrowedDriveEntry::take(drive);

    // The control: this exact directory IS accepted in canonical form, so the
    // rejections below cannot be blamed on the directory itself.
    set_drive_entry(drive, Some(accepted));
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        format!(r"{accepted}\foo"),
        "control: the same directory in canonical form is accepted"
    );

    // Same directory, spellings that are not fully-qualified `X:\...` form.
    // Each names something that exists; each is rejected anyway.
    for spelling in [
        accepted.replace('\\', "/"),
        format!(r"{accepted}\."),
        format!(
            r"{accepted}\..\{}",
            accepted.rsplit('\\').next().unwrap_or("")
        ),
        format!(r"\\?\{accepted}"),
    ] {
        set_drive_entry(drive, Some(&spelling));
        assert_eq!(
            resolve(&format!("{drive}:foo")),
            format!(r"{drive}:\foo"),
            "{spelling:?} names an existing directory but is rejected on shape"
        );
        assert_eq!(
            drive_entry(drive).map(|v| v.to_string_lossy()).as_deref(),
            Some(format!(r"{drive}:\").as_str()),
            "and the rejected entry is written back as the drive root"
        );
    }

    // The type check, which is a separate necessary condition from both the
    // shape above and the existence check in the sibling test. The module doc
    // has listed an existing FILE among the rejections since the drive-entry
    // work, and nothing pinned it -- so the one observation distinguishing
    // "names a directory" from "names something" lived only in prose.
    //
    // Not a file this test creates: `ProbeDir` may be the read-only
    // `%SystemRoot%` fallback, where creating one needs privileges the suite
    // must not assume. This one is present on every Windows host by
    // construction, and setting an entry to a file does not touch the file.
    let system_file = std::path::PathBuf::from(
        std::env::var("SystemRoot").expect("SystemRoot is set on Windows"),
    )
    .join("System32")
    .join("kernel32.dll");
    assert!(
        system_file.is_file(),
        "precondition: the rejected entry must name an existing FILE: {}",
        system_file.display()
    );
    let system_file = system_file.to_str().expect("the system path is UTF-8");

    set_drive_entry(drive, Some(system_file));
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        format!(r"{drive}:\foo"),
        "{system_file} exists and is canonical, and is rejected anyway because \
         it is not a directory -- so existence alone is not the gate"
    );
    assert_eq!(
        drive_entry(drive).map(|v| v.to_string_lossy()).as_deref(),
        Some(format!(r"{drive}:\").as_str()),
        "and an entry naming a file is written back as the drive root too"
    );
}

#[test]
fn a_long_drive_entry_round_trips_through_the_reader() {
    // The reader grows its buffer, and this is what proves it. Windows reports
    // an undersized buffer by returning the REQUIRED capacity rather than the
    // units written, so a reader that treats the two alike slices past its own
    // buffer and panics -- while preserving a legitimate entry, which is the
    // worst moment for it, because the entry is then never restored.
    //
    // 1200 units is comfortably past the 256 the reader starts with and past
    // the 1024 an earlier fixed-size version used, and is a legitimate value: a
    // per-drive entry is a path, and long paths reach far beyond this.
    let drive = probe_drive_from(probe_drives::LONG_ENTRY, None);
    let _restore = BorrowedDriveEntry::take(drive);

    let long = format!(r"C:\{}", "a".repeat(1200));
    set_drive_entry(drive, Some(&long));

    let read_back = drive_entry(drive).expect("the entry was just set");
    assert_eq!(
        read_back.to_string_lossy(),
        long,
        "a long entry survives the read, so the buffer grew instead of truncating"
    );
}

#[test]
fn a_borrowed_drive_entry_is_restored_even_when_the_borrower_panics() {
    // The guard exists for the unwinding path, and a suite that passes never
    // takes it -- so trusting it would mean shipping an untested defence
    // against the exact failure it is there for. This takes the path on
    // purpose.
    let drive = probe_drive_from(probe_drives::BORROW_GUARD, None);

    // The outer guard is not ceremony. This test installs a sentinel to watch
    // the inner guard put back, and without it that install would destroy
    // whatever the process inherited -- so the test for not losing borrowed
    // state would itself lose some. The inner guard still takes the unwinding
    // path; the outer one covers this test's own borrow.
    let _outer = BorrowedDriveEntry::take(drive);
    let sentinel = format!(r"C:\borrowed-entry-{}", std::process::id());
    set_drive_entry(drive, Some(&sentinel));

    let outcome = std::panic::catch_unwind(|| {
        let _restore = BorrowedDriveEntry::take(drive);
        set_drive_entry(drive, Some(r"C:\the-borrowed-value"));
        panic!("expected: this panic exercises the restore-on-unwind path");
    });
    assert!(
        outcome.is_err(),
        "precondition: the borrower must actually panic, or the unwinding path \
         is not the thing being measured"
    );

    assert_eq!(
        drive_entry(drive).map(|v| v.to_string_lossy()).as_deref(),
        Some(sentinel.as_str()),
        "the guard put the entry back while unwinding, where an end-of-test \
         restore would have been skipped"
    );
}

#[test]
fn an_empty_drive_entry_is_distinguished_from_an_absent_one() {
    // **This pins a correction, not a discovery.** The reader used to fold both
    // into `None`, on a recorded measurement that an empty value and an absent
    // name are the same state. They are not; the measurement behind that claim
    // never cleared the last error, so it could only have read whatever an
    // earlier call left behind -- the same "stated more precisely than the
    // evidence reaches" failure this crate keeps meeting, committed inside the
    // comment that called itself measured.
    //
    // The consequence is what makes it worth a test rather than a fix: with the
    // two collapsed, restoring an inherited EMPTY entry deletes it, so the guard
    // written to preserve process state destroys it in exactly one case.
    let drive = probe_drive_from(probe_drives::EMPTY_VS_ABSENT, None);
    let _outer = BorrowedDriveEntry::take(drive);

    set_drive_entry(drive, Some(""));
    let empty = drive_entry(drive);
    assert_eq!(
        empty.as_ref().map(|v| v.to_string_lossy()),
        Some(String::new()),
        "an entry set to the empty string reads back as PRESENT and empty"
    );

    set_drive_entry(drive, None);
    assert_eq!(
        drive_entry(drive),
        None,
        "and a deleted entry reads back as absent, which is the answer the \
         empty one must not be confused with"
    );

    // The two are distinct in the round trip as well as in the read, which is
    // the property restoration actually depends on.
    set_drive_entry_units(drive, empty.as_ref());
    assert_eq!(
        drive_entry(drive).as_ref().map(|v| v.to_string_lossy()),
        Some(String::new()),
        "restoring an empty entry puts back an empty entry, not an absent one"
    );
}
