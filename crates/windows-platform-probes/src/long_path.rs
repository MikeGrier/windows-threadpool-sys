// Copyright (c) Mike Grier.

//! Does the long-path opt-in lift `MAX_PATH` for a **relative** path, and does
//! it change how that path is parsed?
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! # The question, and why reading could not settle it
//!
//! Microsoft's *Maximum Path Length Limitation* puts "relative paths are always
//! limited to a total of MAX_PATH characters" inside the `\\?\` **prefix**
//! section, where it is a consequence of that mechanism -- the prefix cannot be
//! applied to a relative path. Its separate long-path opt-in section says the
//! restriction is removed from a list of functions that includes `CreateFileW`,
//! and excludes nothing. So the documented answer is that the opt-in covers
//! relative paths.
//!
//! That reading produced two wrong answers in one PR #56 review cycle, in
//! opposite directions, which is the reason this exists as a measurement.
//!
//! # The hypothesis this is built to falsify
//!
//! A plausible implementation of the opt-in is to regularize the path and
//! prepend `\\?\` before proceeding as usual. That prefix is precisely what
//! disables `.`, `..` and forward-slash translation -- so if that is how it
//! works, a relative path using any of those could resolve **under** `MAX_PATH`
//! and fail **over** it. A discontinuity at a length boundary is the worst kind
//! to meet in production, and no page states it.
//!
//! So each shape is measured at both lengths. A shape that works short and
//! fails long is the sharp edge; a shape that works at both is evidence the
//! opt-in does not re-parse.
//!
//! # Reading the result
//!
//! Run both binaries. `probe-long-path-aware` carries `longPathAware` in its
//! manifest; `probe-long-path-unaware` is the same code without it, because the
//! un-opted-in case is what most consumers of this workspace actually have.
//! The registry half (`LongPathsEnabled`) is a machine setting and is reported
//! rather than assumed, since a result gathered without it says nothing.
//!
//! **The un-opted-in half is a baseline, not a counter-example**, and its report
//! says so. `MAX_PATH` applying to a process that never opted in is what
//! `MAX_PATH` means; only a refusal with *both* halves in effect would bear on
//! the documented reading. The verdict consults both before drawing any
//! conclusion, so the unaware binary reports what it is -- the case the aware one
//! is read against -- rather than announcing a contradiction it did not test.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GetLastError,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
// `SetCurrentDirectoryW` lives under Environment rather than FileSystem,
// because the current directory is per-process environment state rather than a
// file operation.
use windows_sys::Win32::System::Environment::{GetCurrentDirectoryW, SetCurrentDirectoryW};
use wtf_string::Wtf16String;

/// Windows's classic path ceiling, **counting the terminating NUL**.
const MAX_PATH: usize = 260;

/// The longest path content that fits under the ceiling, terminator excluded.
///
/// The distinction is the whole subject of this probe, so it is spelled out
/// rather than folded into a comparison: a path of exactly `MAX_PATH` content
/// units does *not* fit, because the NUL needs the last one. Comparing against
/// `MAX_PATH` instead would classify that path as under the ceiling while
/// Windows refused it for length -- a row asserting both at once, at exactly the
/// boundary this probe exists to characterize.
///
/// This is the convention the rest of the workspace already states and tests --
/// see `windows-namespace-request-sys` and `windows-file-enumeration-sys`, whose
/// `path` modules define the same pair and assert that the content ceiling is
/// one less than `MAX_PATH`. A probe that measured against a different ceiling
/// than the crates whose designs rest on it would be answering a question nobody
/// asked.
///
/// Public because the report has to *print* it. A column headed `> MAX` next to
/// a module defining `MAX_PATH` as 260 is read as "over 260", and at a resolved
/// length of exactly 260 that reading is wrong in the one place this probe is
/// supposed to be exact. The renderer states the number instead of naming a
/// constant the reader cannot see.
pub const MAX_PATH_CONTENT: usize = MAX_PATH - 1;

/// One directory level of the deep tree. Short, so the depth rather than the
/// width is what carries the length, and free of `.` so no segment is itself a
/// relative operator.
const SEGMENT: &str = "aaaaaaaa";

/// The file every attempt tries to open.
const TARGET: &str = "target.txt";

/// A path shape, and whether it is expected to survive `\\?\` parsing.
///
/// The three differ only in features the prefix disables, which is what makes
/// the comparison a test of the hypothesis rather than of path length alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// Plain backslash-separated segments. Legal under `\\?\` too, so this is
    /// the control: it isolates length from parsing.
    Plain,
    /// Contains `b\..`, which cancels to nothing -- but only if something
    /// resolves it. `\\?\` does not.
    DotDot,
    /// Uses `/` as the separator. Win32 converts it; `\\?\` does not.
    ForwardSlash,
}

impl Shape {
    /// A short word for a table.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::DotDot => "with `..`",
            Self::ForwardSlash => "forward slashes",
        }
    }

    /// Whether `\\?\` parsing would still resolve this shape.
    ///
    /// The prediction the hypothesis makes: if the opt-in prefixes internally,
    /// the two shapes answering `false` here fail once the path grows past
    /// `MAX_PATH`, while `Plain` keeps working.
    #[must_use]
    pub fn survives_verbatim_parsing(self) -> bool {
        matches!(self, Self::Plain)
    }
}

/// What one attempt did.
#[derive(Clone, Debug)]
pub struct Attempt {
    /// The shape tried.
    pub shape: Shape,
    /// Total length the call had to resolve: current directory plus the
    /// relative path, **as written**, not the length of the relative part alone
    /// and not the length after `..` is collapsed.
    ///
    /// That distinction is load-bearing for the `..` shape, whose literal is
    /// five units longer than its canonical form, so it was **measured** rather
    /// than assumed: forcing the deep level to 21 on the development host put
    /// plain at 258 and `..` at 263 against a content ceiling of 259, and in a
    /// binary with no `longPathAware` manifest plain **opened** while `..` was
    /// **refused**. Had Windows collapsed `..` before applying the ceiling, both
    /// would have been 258 and both would have opened. So the length Windows
    /// compares is the one written, and this is that number.
    ///
    /// The measurement above is the evidence, and it is self-contained: it uses
    /// this crate's own un-manifested binary, so it does not depend on any
    /// assumption about some other program's manifest.
    ///
    /// That independence is the point, because the obvious shortcut is unsound.
    /// Reaching for `cmd.exe` to try a long path measures whatever `cmd`'s
    /// manifest says, not the un-opted-in case: on the development host --
    /// Windows 11 build 26200, `cmd.exe` 10.0.26100.1 -- `cmd` carries
    /// `longPathAware` in its own manifest, beside `dpiAware`, so a path that
    /// opens there says nothing about the ceiling. Stated with the build because
    /// it is a fact about that binary on that host rather than about `cmd`
    /// forever: it has not always been so, and a probe that assumed it either way
    /// would be resting on someone else's manifest instead of measuring.
    ///
    /// **In UTF-16 code units**, which is the unit `MAX_PATH` itself is
    /// expressed in. Counting Rust's platform encoding instead would disagree
    /// the moment a non-ASCII character appeared in the temporary directory's
    /// path, and would put an attempt on the wrong side of the ceiling.
    pub resolved_len: usize,
    /// Whether that total is too long to fit under the ceiling.
    ///
    /// That is `> MAX_PATH_CONTENT` (259), **not** `> MAX_PATH` (260): a path of
    /// exactly 260 content units does not fit, because the terminator needs the
    /// last one. The field name is older than the distinction and is kept for the
    /// column it feeds; the comparison is the one the sibling crates make.
    pub over_max_path: bool,
    /// Whether `CreateFileW` opened the file.
    pub opened: bool,
    /// The Win32 error when it did not.
    pub error: u32,
}

/// Everything one run observed.
#[derive(Clone, Debug)]
pub struct Observation {
    /// Whether this binary declares `longPathAware`.
    pub manifest_aware: bool,
    /// Whether the machine has `LongPathsEnabled` set to 1.
    /// `None` when the run was refused before the registry could be read.
    ///
    /// A `bool` cannot say "not consulted", and the difference matters: reading
    /// an unconsulted flag as `false` made a refused run report `LongPathsEnabled
    /// : unset or 0` and "the machine half of the opt-in is absent" on a host
    /// where it is set to 1 -- a measured-sounding claim about a query that was
    /// never issued. The refusal has to come first, because reading the registry
    /// spawns a process and that is one of the calls that hangs, so the honest
    /// answer is a third state rather than a default.
    pub registry_enabled: Option<bool>,
    /// Every attempt, short ones first.
    pub attempts: Vec<Attempt>,
    /// Set when the apparatus itself failed, in which case the attempts say
    /// nothing about the machine.
    pub apparatus_error: Option<String>,
}

/// A null-terminated wide string, as Win32 wants.
fn wide(path: &OsStr) -> Vec<u16> {
    path.encode_wide().chain(std::iter::once(0)).collect()
}

/// Read `LongPathsEnabled`, which is half the opt-in and is a machine setting
/// rather than anything this process controls.
///
/// Reported rather than assumed: a run on a machine without it measures the
/// un-opted-in case whatever the manifest says, and reading the answer as
/// though the opt-in were active would invert the conclusion.
#[must_use]
pub fn registry_enabled() -> bool {
    // Read through `reg.exe` rather than taking a registry dependency for one
    // value in a probe. A missing key, a non-zero exit and an unparsable value
    // all mean the same thing here: not enabled.
    std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SYSTEM\CurrentControlSet\Control\FileSystem",
            "/v",
            "LongPathsEnabled",
        ])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| enabled_in(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(false)
}

/// Whether `reg query`'s output says the machine half of the opt-in is on.
///
/// Separated from the spawn so the reading is testable against captured output
/// rather than against whatever the developing machine happens to be set to.
///
/// Any nonzero value counts as enabled: this is a boolean flag stored in a
/// DWORD, so the value that is not zero is the one that means yes.
fn enabled_in(stdout: &str) -> bool {
    registry_dword(stdout, "LongPathsEnabled").is_some_and(|value| value != 0)
}

/// The DWORD `reg query ... /v <name>` printed, if it printed one.
///
/// Parsed as a whole token rather than searched for as a substring. `reg.exe`
/// prints the value in hex, so a substring test for `0x1` also matches `0x10`
/// and every other value that merely starts that way -- which would report a
/// machine as opted in on the strength of an unrelated setting.
fn registry_dword(stdout: &str, name: &str) -> Option<u32> {
    stdout.lines().find_map(|line| {
        let mut tokens = line.split_whitespace();
        // `reg.exe` prints `<name>    REG_DWORD    0x1`, and this probe queries
        // one value, so a line that does not have that shape is not the answer.
        if tokens.next()? != name || tokens.next()? != "REG_DWORD" {
            return None;
        }
        let digits = tokens.next()?.strip_prefix("0x")?;
        u32::from_str_radix(digits, 16).ok()
    })
}

/// Create one directory by absolute `\\?\` path, so building the apparatus
/// never depends on the behaviour under test.
fn create_dir_verbatim(path: &Path) -> Result<(), String> {
    let verbatim = PathBuf::from(format!(r"\\?\{}", path.display()));
    let wide = wide(verbatim.as_os_str());
    // SAFETY: `wide` is a live null-terminated buffer for the duration of the
    // call, and a null security descriptor requests the default.
    let created = unsafe { CreateDirectoryW(wide.as_ptr(), std::ptr::null()) };
    if created == 0 {
        // SAFETY: called immediately after the failing call.
        let error = unsafe { GetLastError() };
        // Already there is success for our purposes: the apparatus is a shape on
        // disk, not a thing this run must be the one to have created.
        if error != ERROR_ALREADY_EXISTS {
            return Err(format!("CreateDirectoryW({verbatim:?}) failed: {error}"));
        }
    }
    Ok(())
}

/// Build a directory chain `depth` levels deep under `root`, returning the
/// relative path that reaches the bottom.
fn build_tree(root: &Path, depth: usize) -> Result<PathBuf, String> {
    let mut absolute = root.to_path_buf();
    let mut relative = PathBuf::new();
    for _ in 0..depth {
        absolute.push(SEGMENT);
        relative.push(SEGMENT);
        create_dir_verbatim(&absolute)?;
    }
    Ok(relative)
}

/// Write the target file at the bottom of the chain, by absolute `\\?\` path.
fn create_target(bottom: &Path) -> Result<(), String> {
    let verbatim = PathBuf::from(format!(r"\\?\{}\{TARGET}", bottom.display()));
    let wide = wide(verbatim.as_os_str());
    // SAFETY: `wide` is live and null-terminated; the handle is closed below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: called immediately after the failing call.
        return Err(format!("could not create the target: {}", unsafe {
            GetLastError()
        }));
    }
    // SAFETY: `handle` is a live handle this function just opened.
    unsafe { CloseHandle(handle) };
    Ok(())
}

/// Render one relative path of the requested shape reaching `depth` levels down.
fn relative_path(depth: usize, shape: Shape) -> String {
    let mut parts: Vec<String> = (0..depth).map(|_| SEGMENT.to_string()).collect();
    match shape {
        Shape::Plain | Shape::ForwardSlash => {}
        Shape::DotDot => {
            // A descent that immediately cancels. Placed at the bottom so the
            // path is at its longest when the operator appears -- the position
            // where a prefix-then-parse implementation would be least able to
            // resolve it.
            parts.push("b".to_string());
            parts.push("..".to_string());
        }
    }
    parts.push(TARGET.to_string());
    let separator = if shape == Shape::ForwardSlash {
        "/"
    } else {
        r"\"
    };
    parts.join(separator)
}

/// Try to open the target through one relative path, from the current
/// directory, with no prefix of any kind.
fn attempt(current_dir_len: usize, depth: usize, shape: Shape) -> Attempt {
    let relative = relative_path(depth, shape);
    // Plus one for the separator Windows inserts when it joins the two. Both
    // lengths are UTF-16 code units, which is the unit `MAX_PATH` is expressed
    // in -- see `current_dir_len`'s construction in `measure`. The relative
    // part is built from ASCII constants here, so its two counts agree today;
    // it is measured the same way regardless, because a unit that is only
    // correct while the input happens to be ASCII is one waiting to be wrong.
    let resolved_len = current_dir_len + 1 + Wtf16String::from_os_str(OsStr::new(&relative)).len();
    let wide = wide(OsStr::new(&relative));
    // SAFETY: `wide` is a live null-terminated buffer for the duration of the
    // call; the handle, if any, is closed below.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let opened = handle != INVALID_HANDLE_VALUE;
    let error = if opened {
        // SAFETY: `handle` is a live handle this call just opened.
        unsafe { CloseHandle(handle) };
        0
    } else {
        // SAFETY: called immediately after the failing call.
        unsafe { GetLastError() }
    };
    Attempt {
        shape,
        resolved_len,
        over_max_path: resolved_len > MAX_PATH_CONTENT,
        opened,
        error,
    }
}

/// This process's current directory, as a null-terminated wide string.
///
/// Sized then fetched, which is this API's documented shape: a zero length with
/// a null buffer returns the size *including* the terminator, and the filling
/// call returns the count *excluding* it.
fn current_directory() -> Result<Vec<u16>, String> {
    // SAFETY: the documented sizing form -- a zero length with a null buffer,
    // which writes nothing and returns the required size.
    let needed = unsafe { GetCurrentDirectoryW(0, std::ptr::null_mut()) };
    if needed == 0 {
        // SAFETY: called immediately after the failing call.
        return Err(format!("GetCurrentDirectoryW sizing failed: {}", unsafe {
            GetLastError()
        }));
    }
    let mut buffer = vec![0_u16; needed as usize];
    // SAFETY: `buffer` has `needed` elements, which is the size the call above
    // asked for, and is writable for that length.
    let written = unsafe { GetCurrentDirectoryW(needed, buffer.as_mut_ptr()) };
    if written == 0 || written >= needed {
        // SAFETY: called immediately after the failing call.
        return Err(format!("GetCurrentDirectoryW failed: {}", unsafe {
            GetLastError()
        }));
    }
    Ok(buffer)
}

/// The temporary tree and the process state this experiment borrows.
///
/// **A guard, because both are leaks if `measure` returns early**, and it
/// returns early on every apparatus failure. This is a library function, so
/// neither is excused by the probe binaries exiting straight afterwards: a test
/// or any other caller keeps running in the process whose current directory was
/// moved.
///
/// The tree is the sharper of the two. It is deliberately longer than
/// `MAX_PATH`, which is the very property that stops Explorer and `del` from
/// removing it -- so litter left in `%TEMP%` by a probe about long paths is
/// litter that is hard to clear up by hand.
struct Apparatus {
    root: PathBuf,
    /// Where the process was before [`Self::enter`], if it moved at all.
    previous_directory: Option<Vec<u16>>,
}

impl Apparatus {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            previous_directory: None,
        }
    }

    /// Move the process into `directory`, remembering where it was.
    fn enter(&mut self, directory: &Path) -> Result<(), String> {
        // Captured *before* the move, or there is nothing to go back to.
        let previous = current_directory()?;
        let wide = wide(directory.as_os_str());
        // SAFETY: `wide` is a live null-terminated buffer for the call.
        if unsafe { SetCurrentDirectoryW(wide.as_ptr()) } == 0 {
            // SAFETY: called immediately after the failing call.
            return Err(format!("SetCurrentDirectoryW failed: {}", unsafe {
                GetLastError()
            }));
        }
        self.previous_directory = Some(previous);
        Ok(())
    }
}

impl Drop for Apparatus {
    fn drop(&mut self) {
        // **Restore the directory first, and that ordering is load-bearing.**
        // A process's current directory holds a handle on it, so removing the
        // tree while parked inside it fails -- the cleanup would silently do
        // nothing and leave exactly the litter this guard exists to prevent.
        if let Some(previous) = &self.previous_directory {
            // SAFETY: `previous` is the null-terminated buffer
            // `GetCurrentDirectoryW` filled, still live here.
            unsafe { SetCurrentDirectoryW(previous.as_ptr()) };
        }

        // By verbatim path, for the same reason the tree was built by one: the
        // deep branch is past `MAX_PATH`, so an ordinary path would fail to
        // reach it on a host that has not opted in -- which is half the hosts
        // this probe is meant to run on.
        let verbatim = PathBuf::from(format!(r"\\?\{}", self.root.display()));
        // Best-effort: a failure here leaves litter, which is worth neither a
        // panic in a `Drop` nor a field on an observation about path lengths.
        let _ = std::fs::remove_dir_all(&verbatim);
    }
}

/// Run the experiment.
///
/// `manifest_aware` is what the *caller* knows about its own manifest -- the
/// process cannot ask Windows whether it opted in, so the two binaries pass
/// their own answer and are named for it.
///
/// The temporary tree and the current directory are both restored before this
/// returns, on every path including the apparatus failures -- see `Apparatus`.
///
/// # Not safe to call concurrently
///
/// This borrows **process-wide** state: it moves the current directory, and it
/// builds its tree under a root named after the process id. Two calls at once
/// in one process share both -- one would remove the tree the other was still
/// using, and they would fight over the directory. A unique root per call would
/// not fix that, because there is one current directory per process however the
/// trees are named.
///
/// The probe binaries call this once and exit, so this costs them nothing. A
/// caller running it from a test suite must serialize its calls; the tests
/// beside this module do exactly that.
#[must_use]
pub fn measure(manifest_aware: bool) -> Observation {
    // First, before anything at all: both `temp_dir()` and the `reg.exe` spawn
    // below hang on an over-long temporary directory, so neither may run first.
    // `registry_enabled` is `None` here rather than `false`: the query was never
    // issued, and saying "not enabled" would be a claim about the machine.
    if let Some(error) = temp_dir_refusal(
        std::env::var_os("TMP").as_deref(),
        std::env::var_os("TEMP").as_deref(),
    ) {
        return Observation {
            manifest_aware,
            registry_enabled: None,
            attempts: Vec::new(),
            apparatus_error: Some(error),
        };
    }

    let mut observation = Observation {
        manifest_aware,
        registry_enabled: Some(registry_enabled()),
        attempts: Vec::new(),
        apparatus_error: None,
    };

    let root = std::env::temp_dir().join(format!("long-path-probe-{}", std::process::id()));
    if let Err(error) = create_dir_verbatim(&root) {
        observation.apparatus_error = Some(error);
        return observation;
    }
    // From here on the tree exists, so every exit below has something to clean
    // up -- including the early returns, which is why this is a guard.
    let mut apparatus = Apparatus::new(root.clone());

    // Deep enough that the resolved path clears `MAX_PATH` with room to spare,
    // and shallow enough that the short case stays well under it.
    let deep = 40;
    let shallow = 1;

    // Only the side effect is wanted. Each attempt spells its own relative path,
    // because the spelling is what is under test.
    if let Err(error) = build_tree(&root, deep) {
        observation.apparatus_error = Some(error);
        return observation;
    }
    // `b`, for the `..` shape to descend into and immediately leave.
    for depth in [shallow, deep] {
        let mut bottom = root.clone();
        for _ in 0..depth {
            bottom.push(SEGMENT);
        }
        if let Err(error) = create_dir_verbatim(&bottom.join("b")) {
            observation.apparatus_error = Some(error);
            return observation;
        }
        if let Err(error) = create_target(&bottom) {
            observation.apparatus_error = Some(error);
            return observation;
        }
    }

    // The current directory is the short root for every attempt, so the length
    // under test lives in the relative path rather than in the cwd.
    if let Err(error) = apparatus.enter(&root) {
        observation.apparatus_error = Some(error);
        return observation;
    }
    // **UTF-16 code units, not bytes.** `MAX_PATH` counts what Windows counts,
    // and `OsStr::len` counts Rust's platform encoding -- which is WTF-8 here,
    // so a non-ASCII character in `%TEMP%` makes the two disagree and can put
    // an attempt on the wrong side of the ceiling in the report. `Wtf16String`
    // is the workspace's own answer to exactly this: it holds the string in the
    // encoding Windows uses, so its `len` is the number under test rather than
    // a conversion of one.
    let current_dir_len = Wtf16String::from_os_str(root.as_os_str()).len();

    for depth in [shallow, deep] {
        for shape in [Shape::Plain, Shape::DotDot, Shape::ForwardSlash] {
            observation
                .attempts
                .push(attempt(current_dir_len, depth, shape));
        }
    }

    observation
}

/// The longest temporary directory this probe will run in, **in UTF-16 units**.
///
/// A chosen limit, not a derived one. A long enough temporary directory makes a
/// `longPathAware` process hang rather than fail, and this sits far enough short
/// of that to not care where exactly it starts.
const MAX_TEMP_DIR: usize = 200;

/// Why this run must refuse to start, if it must, given `%TMP%` and `%TEMP%`.
///
/// Runs before anything that could resolve a path, because the failure it avoids
/// is a hang: there is nothing to check afterwards when the call never returns.
///
/// The order mirrors `GetTempPath`: `%TMP%` first, then `%TEMP%`, then fallbacks
/// that are always short. Checking both unconditionally would refuse a run whose
/// `%TMP%` is perfectly usable merely because a stale `%TEMP%` sits beside it.
fn temp_dir_refusal(tmp: Option<&OsStr>, temp: Option<&OsStr>) -> Option<String> {
    let (name, value) = match (tmp, temp) {
        (Some(value), _) => ("TMP", value),
        (None, Some(value)) => ("TEMP", value),
        (None, None) => return None,
    };

    // The apparatus is built through `\\?\` paths so that creating it never
    // depends on the behaviour under test, and this crate composes that prefix by
    // concatenation. Three shapes of temporary directory make that composition
    // wrong rather than merely long, and all are refused for the same reason the
    // length is: the probe would report an apparatus failure, or measure a path
    // that is not the one it names, and either way say nothing about the ceiling.
    //
    // Checked in this order because each check needs the previous one to have
    // passed to be able to say anything true: the prefix tests read the value as
    // text, and reading an ill-formed value as text is exactly the substitution
    // the second refusal exists to prevent. Classifying first and validating
    // afterwards would refuse an ill-formed value under whichever prefix its
    // replacement characters happened to spell.

    // `Path::display` substitutes U+FFFD for an unpaired surrogate, so a name
    // containing one would compose a verbatim path naming a different file --
    // silently, and in the apparatus rather than in the measurement.
    let Some(text) = value.to_str() else {
        return Some(format!(
            "%{name}% is not well-formed UTF-16. The apparatus composes its `\\\\?\\` \
             paths as text, which would replace the ill-formed part and name a \
             different file. Point %{name}% somewhere expressible."
        ));
    };

    // `\\?\` and `\\.\` open with two backslashes but are the device namespace,
    // not UNC, so they are separated out ahead of the UNC test rather than
    // reported as a server share the machine does not have. The refusal is not
    // only about the doubled prefix: `\\?\` turns off the path normalisation that
    // this probe exists to measure, so a run rooted there would measure the
    // verbatim path's ceiling and label it the ordinary one.
    if let Some(prefix) = [r"\\?\", r"\\.\"]
        .into_iter()
        .find(|prefix| text.starts_with(prefix))
    {
        return Some(format!(
            "%{name}% starts with `{prefix}`, which names the device namespace rather \
             than an ordinary directory. This probe composes its own `\\\\?\\` prefix by \
             concatenation, and `\\\\?\\` additionally turns off the path normalisation \
             the probe measures. Point %{name}% at an ordinary local directory."
        ));
    }

    // A UNC root needs `\\?\UNC\server\share`, not `\\?\` glued to `\\server`.
    // Supporting it properly is not the problem -- it is three lines -- but it
    // could not be exercised on any machine this workspace is developed or tested
    // on, and an untested path through the apparatus is worth less than an honest
    // refusal.
    if text.starts_with(r"\\") {
        return Some(format!(
            "%{name}% is a UNC path. This probe builds its apparatus through `\\\\?\\` \
             paths, which spell a UNC root differently, and it does not implement that \
             spelling. Point %{name}% at a local directory."
        ));
    }

    // UTF-16 units, the unit Windows counts, for the reason `resolved_len`
    // records: a non-ASCII character makes a byte count disagree.
    let units = Wtf16String::from_os_str(value).len();
    if units <= MAX_TEMP_DIR {
        return None;
    }

    Some(format!(
        "%{name}% is {units} UTF-16 units, over this probe's limit of {MAX_TEMP_DIR}. \
         A temporary directory that long makes a longPathAware process hang rather \
         than fail, so the run is refused instead. Point %{name}% somewhere shorter."
    ))
}

/// Whether an error means "the path was rejected for length", as opposed to a
/// genuine absence.
///
/// Windows reports an over-long path as `ERROR_PATH_NOT_FOUND` rather than
/// anything length-specific, which is why the apparatus creates every target
/// first: a `NOT_FOUND` from a file that provably exists is the length refusal.
#[must_use]
pub fn is_refusal(attempt: &Attempt) -> bool {
    !attempt.opened && matches!(attempt.error, ERROR_PATH_NOT_FOUND | ERROR_FILE_NOT_FOUND)
}

#[cfg(test)]
mod tests;
