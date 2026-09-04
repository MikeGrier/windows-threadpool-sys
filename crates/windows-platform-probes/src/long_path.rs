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

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GetLastError, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
// `SetCurrentDirectoryW` lives under Environment rather than FileSystem,
// because the current directory is per-process environment state rather than a
// file operation.
use windows_sys::Win32::System::Environment::SetCurrentDirectoryW;

/// Windows's classic path ceiling.
const MAX_PATH: usize = 260;

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
    /// relative path. This is the number `MAX_PATH` is compared against, not
    /// the length of the relative part alone.
    pub resolved_len: usize,
    /// Whether that total exceeds `MAX_PATH`.
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
    pub registry_enabled: bool,
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
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("0x1"))
        .unwrap_or(false)
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
        // 183 is ERROR_ALREADY_EXISTS, which is success for our purposes.
        if error != 183 {
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
    // Plus one for the separator Windows inserts when it joins the two.
    let resolved_len = current_dir_len + 1 + relative.len();
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
        over_max_path: resolved_len > MAX_PATH,
        opened,
        error,
    }
}

/// Run the experiment.
///
/// `manifest_aware` is what the *caller* knows about its own manifest -- the
/// process cannot ask Windows whether it opted in, so the two binaries pass
/// their own answer and are named for it.
#[must_use]
pub fn measure(manifest_aware: bool) -> Observation {
    let mut observation = Observation {
        manifest_aware,
        registry_enabled: registry_enabled(),
        attempts: Vec::new(),
        apparatus_error: None,
    };

    let root = std::env::temp_dir().join(format!("long-path-probe-{}", std::process::id()));
    if let Err(error) = create_dir_verbatim(&root) {
        observation.apparatus_error = Some(error);
        return observation;
    }

    // Deep enough that the resolved path clears `MAX_PATH` with room to spare,
    // and shallow enough that the short case stays well under it.
    let deep = 40;
    let shallow = 1;

    let deep_relative = match build_tree(&root, deep) {
        Ok(relative) => relative,
        Err(error) => {
            observation.apparatus_error = Some(error);
            return observation;
        }
    };
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
    let _ = deep_relative;

    // The current directory is the short root for every attempt, so the length
    // under test lives in the relative path rather than in the cwd.
    let root_wide = wide(root.as_os_str());
    // SAFETY: `root_wide` is a live null-terminated buffer for the call.
    if unsafe { SetCurrentDirectoryW(root_wide.as_ptr()) } == 0 {
        // SAFETY: called immediately after the failing call.
        observation.apparatus_error = Some(format!("SetCurrentDirectoryW failed: {}", unsafe {
            GetLastError()
        }));
        return observation;
    }
    let current_dir_len = root.as_os_str().len();

    for depth in [shallow, deep] {
        for shape in [Shape::Plain, Shape::DotDot, Shape::ForwardSlash] {
            observation
                .attempts
                .push(attempt(current_dir_len, depth, shape));
        }
    }

    observation
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
