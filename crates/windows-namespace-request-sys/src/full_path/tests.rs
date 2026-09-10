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
fn a_drive_relative_path_is_rooted_at_that_drive_and_not_the_process_directory() {
    // The third rooting form. A drive-relative path is rooted at *that drive's*
    // current directory -- and the rule has two arms, which is the part that
    // gets missed:
    //
    //   * For a drive OTHER than the current one, Windows reads the hidden
    //     `=X:` entry recorded for it.
    //   * For the CURRENT drive the entry is ignored entirely and the process
    //     current directory wins. Measured: setting `=Q:` while the process is
    //     on `Q:` changes nothing.
    //
    // **This test does not mutate `=X:`, but the call it exercises may.**
    // Measured: resolving `X:foo` for a non-current drive validates that
    // drive's entry against the filesystem and WRITES it to `X:\` when it does
    // not name an existing directory -- creating it when absent, so merely
    // running this test changes the process environment on ANY host, not just
    // one carrying a stale entry. That is
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
    let other = match cwd_drive {
        Some(d) if d.eq_ignore_ascii_case(&'X') => 'Y',
        _ => 'X',
    };
    let resolved = resolve(&format!("{other}:foo"));

    // Compared case-insensitively, because the case is not this test's to
    // choose: the letter comes back as Windows recorded it, not as it was
    // typed. This host returns `q:\...` for an uppercase `Q:` input, because the
    // shell was started with a lowercase `cd`. An earlier version compared bytes
    // and would have failed on a drive visited in lowercase.
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
            "on the current drive, the per-drive entry is ignored and the \
             process directory is used"
        );
    }
}

#[test]
fn trailing_dots_and_spaces_are_trimmed_from_ordinary_components() {
    // The module doc says the rewrite trims trailing dots and spaces. Until now
    // that was only exercised through a final `.` component (which is the
    // separate `.`-collapsing rule) and through device spellings (which take
    // the short-circuit and never reach the ordinary path). Neither pins this.
    //
    // Every case measured before being written down.
    for (input, expected) in [
        (r"C:\name.", r"C:\name"),
        (r"C:\name ", r"C:\name"),
        (r"C:\name...", r"C:\name"),
        (r"C:\name   ", r"C:\name"),
        (r"C:\name. ", r"C:\name"),
        // Trimming applies per component, not only at the end of the path.
        (r"C:\a.\b", r"C:\a\b"),
        // An extension is not special: the trailing dot goes, the rest stays.
        (r"C:\name.txt.", r"C:\name.txt"),
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
    // `.\CON` is excluded deliberately -- the `.` component collapses, so its
    // expected form is the bare name, which the loop below would have to
    // special-case. It is covered by the device-negative test instead.
    let base = current_directory();
    let base = base.trim_end_matches('\\');
    for name in ["CON.txt", "CONIN", "COM0", "COM10", r"a\CON"] {
        assert_eq!(
            resolve(name),
            format!(r"{base}\{name}"),
            "{name:?} is not a bare device name, so it roots under the current \
             directory rather than merely avoiding the device namespace"
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
/// case, so [`ProbeDir::created`] tells the caller whether to clean up.
struct ProbeDir {
    path: std::path::PathBuf,
    created: bool,
}

fn probe_directory(tag: &str) -> ProbeDir {
    let drive_rooted = |p: &std::path::Path| {
        let s = p.as_os_str().to_string_lossy().into_owned();
        let mut chars = s.chars();
        matches!(
            (chars.next(), chars.next(), chars.next()),
            (Some(d), Some(':'), Some('\\')) if d.is_ascii_alphabetic()
        )
    };

    let temp = std::env::temp_dir();
    if drive_rooted(&temp) {
        let path = temp.join(format!("wnrs-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create the probe directory");
        return ProbeDir {
            path,
            created: true,
        };
    }

    // Not creating anything here, so no write permission is needed on a host
    // whose temp directory is redirected off a drive letter.
    let system_root = std::env::var("SystemRoot").expect("SystemRoot is always set on Windows");
    let path = std::path::PathBuf::from(system_root);
    assert!(
        drive_rooted(&path),
        "the fallback probe must be drive-rooted: {}",
        path.display()
    );
    ProbeDir {
        path,
        created: false,
    }
}
/// A drive letter to probe with, which is certainly not the current drive.
///
/// **The letter cannot be a constant, for the reason these tests exist.** An
/// entry is only consulted for a drive *other* than the current one -- on the
/// current drive it is ignored and the process directory wins -- so a test that
/// hard-codes `W` asserts something false when run from `W:`. Measured: with
/// `subst W:` and the suite launched from `W:\`, the verbatim test failed.
/// `W` and `V` are exactly the letters a mapped network drive or a `subst`
/// tends to take.
///
/// Each caller passes a disjoint pair, so two tests can never land on the same
/// letter and race under libtest's thread-per-test model.
fn probe_drive(preferred: char, fallback: char) -> char {
    let cwd = current_directory();
    match cwd.chars().next().filter(char::is_ascii_alphabetic) {
        Some(current) if current.eq_ignore_ascii_case(&preferred) => fallback,
        // A UNC current directory has no drive letter, so nothing collides.
        _ => preferred,
    }
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
            return None;
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
    assert!(ok != 0, "set ={drive}: entry");
}

#[test]
fn a_drive_relative_path_uses_that_drives_entry_verbatim_and_rewrites_a_bad_one() {
    // The arm the sibling test can only BOUND. Without controlling the entry,
    // an implementation that always used the drive root would satisfy every
    // assertion there, because a host with no `=X:` entry cannot tell the two
    // rules apart.
    //
    // **Controlling it is not the hazard it looks like**, and that is what
    // unblocked this test. The objection was that `=X:` is process-global while
    // these tests share a process -- but `GetFullPathNameW` *itself* writes the
    // entry on every drive-relative resolution, creating it when absent. The
    // code under test already mutates this state, so a test that sets it first
    // introduces no hazard that resolving alone did not.
    //
    // `W` (or `U` when the suite runs from `W:`) keeps this clear of the
    // sibling tests' `X`/`Y` and `V`/`T`, so none of them can race.
    let drive = probe_drive('W', 'U');
    let restore = drive_entry(drive);

    // A real directory that is certainly NOT on the probe drive, which is what
    // makes it the right probe: if the entry is honoured verbatim, a
    // drive-relative path resolves onto another drive entirely.
    let probe_dir = probe_directory("verbatim");
    let probe = probe_dir.path.to_str().expect("the probe path is UTF-8");

    set_drive_entry(drive, Some(probe));
    assert_eq!(
        resolve(&format!("{drive}:foo")),
        format!(r"{probe}\foo"),
        "an entry naming an existing directory is honoured verbatim, even onto \
         a different drive -- so \"that drive's own current directory\" is the \
         convention the entry usually holds, not a guarantee about the result"
    );

    // An entry that does not name an existing directory is rejected, and the
    // call rewrites it to the drive root rather than leaving it stale.
    set_drive_entry(drive, Some(r"C:\no-such-directory-for-this-test"));
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

    set_drive_entry_units(drive, restore.as_ref());
    if probe_dir.created {
        let _ = std::fs::remove_dir(&probe_dir.path);
    }
}

#[test]
fn a_rejected_drive_entry_is_replaced_by_the_drive_root() {
    // Acceptance needs BOTH a shape and an existence check, and a draft of the
    // module doc claimed it was "a filesystem query rather than a syntax test"
    // -- having measured only the existence half. Every value below names an
    // existing directory, so anything rejected here is rejected on shape alone.
    //
    // Pinned because the distinction is not guessable and the doc asserts it.
    let drive = probe_drive('V', 'T');
    let restore = drive_entry(drive);

    let probe_dir = probe_directory("shape");
    let accepted = probe_dir.path.to_str().expect("the probe path is UTF-8");

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

    set_drive_entry_units(drive, restore.as_ref());
    if probe_dir.created {
        let _ = std::fs::remove_dir(&probe_dir.path);
    }
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
    let drive = probe_drive('R', 'S');
    let restore = drive_entry(drive);

    let long = format!(r"C:\{}", "a".repeat(1200));
    set_drive_entry(drive, Some(&long));

    let read_back = drive_entry(drive).expect("the entry was just set");
    assert_eq!(
        read_back.to_string_lossy(),
        long,
        "a long entry survives the read, so the buffer grew instead of truncating"
    );

    set_drive_entry_units(drive, restore.as_ref());
}
