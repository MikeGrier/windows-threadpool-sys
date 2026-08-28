// Copyright (c) Mike Grier.

//! Where directory-enumeration state lives, and what a duplicated handle does
//! and does not share.
//!
//! A handle is a reference to a kernel object, so duplicating one shares that
//! object rather than cloning it. Directory enumeration keeps its cursor in the
//! file object, which makes "does a duplicate share the cursor?" a question with
//! consequences for any design that marshals a handle to another thread. It is
//! measured here rather than reasoned from the object model, because handles are
//! usually plain refcounted references and occasionally are not.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, FALSE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_ID_EXTD_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileBasicInfo, FileIdExtdDirectoryInfo, FileIdExtdDirectoryRestartInfo,
    FileIdInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Small on purpose.
///
/// A buffer large enough to drain the fixture in one call would answer none of
/// these questions, because the cursor would never be left mid-directory.
const BUFFER_BYTES: usize = 320;

/// Enough entries that the small buffer needs several calls.
const FIXTURE_FILES: usize = 14;

/// A temporary directory that removes itself.
///
/// Named per process and per label so concurrent tests cannot collide.
pub struct Fixture {
    path: PathBuf,
}

impl Fixture {
    /// Create a directory of predictably named files.
    ///
    /// # Panics
    ///
    /// Panics if the directory or its files cannot be created, since a probe
    /// that silently measured nothing would be worse than one that stopped.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "windows-platform-probes-{}-{label}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the probe fixture directory");
        for index in 0..FIXTURE_FILES {
            std::fs::write(path.join(format!("f{index:02}.t")), b"x")
                .expect("write a fixture file");
        }
        Self { path }
    }

    /// The directory's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// An owned directory handle.
pub struct DirHandle(HANDLE);

impl DirHandle {
    /// Open `path` for listing.
    ///
    /// # Panics
    ///
    /// Panics if the open fails; every caller here has just created the target.
    #[must_use]
    pub fn open(path: &Path) -> Self {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the remaining
        // arguments are plain values and null pointers the API accepts.
        let raw = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        assert!(
            raw != INVALID_HANDLE_VALUE,
            "opening the fixture directory failed: {}",
            // SAFETY: no preconditions.
            unsafe { GetLastError() }
        );
        Self(raw)
    }

    /// Duplicate this handle within the current process.
    ///
    /// # Panics
    ///
    /// Panics if duplication fails, which would make the probe vacuous.
    #[must_use]
    pub fn duplicate(&self) -> Self {
        let mut out: HANDLE = std::ptr::null_mut();
        // SAFETY: `self.0` is live, `out` is a valid writable destination, and
        // both process handles are pseudo-handles that need no cleanup.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.0,
                GetCurrentProcess(),
                &mut out,
                0,
                FALSE,
                DUPLICATE_SAME_ACCESS,
            )
        };
        assert!(ok != 0, "DuplicateHandle failed: {}", unsafe {
            GetLastError()
        });
        Self(out)
    }

    /// One enumeration call, returning the names it produced.
    ///
    /// `Err` carries the Win32 code, which is how exhaustion is reported.
    pub fn enumerate(&self, restart: bool) -> Result<Vec<String>, u32> {
        let class = if restart {
            FileIdExtdDirectoryRestartInfo
        } else {
            FileIdExtdDirectoryInfo
        };
        let mut words = vec![0u64; BUFFER_BYTES / 8];
        let capacity = u32::try_from(words.len() * 8).expect("a fixed small buffer fits a u32");
        // SAFETY: the buffer is 8-byte aligned by construction (the element type
        // is `u64`), which the directory-information classes require, and its
        // capacity is reported honestly.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.0,
                class,
                words.as_mut_ptr().cast::<c_void>(),
                capacity,
            )
        };
        if ok == 0 {
            // SAFETY: no preconditions.
            return Err(unsafe { GetLastError() });
        }
        let mut names = Vec::new();
        let base = words.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            // SAFETY: the call above filled the buffer with a chain of records,
            // each beginning at the previous record's next-entry offset, and the
            // walk stops at the terminator the API writes.
            let record = unsafe { &*(base.add(offset).cast::<FILE_ID_EXTD_DIR_INFO>()) };
            let units = record.FileNameLength as usize / 2;
            let name_ptr = std::ptr::addr_of!(record.FileName).cast::<u16>();
            // SAFETY: the record's own length field bounds the name.
            let name = unsafe { std::slice::from_raw_parts(name_ptr, units) };
            names.push(String::from_utf16_lossy(name));
            let next = record.NextEntryOffset as usize;
            if next == 0 {
                break;
            }
            offset += next;
        }
        Ok(names)
    }

    /// Every remaining name, from the handle's current position.
    #[must_use]
    pub fn drain(&self) -> Vec<String> {
        let mut all = Vec::new();
        while let Ok(mut names) = self.enumerate(false) {
            all.append(&mut names);
        }
        all
    }
}

impl Drop for DirHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned and closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Which single-shot query to interleave with an enumeration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SingleShot {
    /// `GetFileInformationByHandleEx` with `FileBasicInfo`.
    BasicInfo,
    /// `GetFileInformationByHandleEx` with `FileIdInfo`.
    IdInfo,
    /// The non-`Ex` `GetFileInformationByHandle`.
    NonEx,
}

impl SingleShot {
    /// Perform the query. Returns whether it succeeded.
    fn run(self, handle: &DirHandle) -> bool {
        match self {
            Self::BasicInfo => {
                // SAFETY: the destination is a valid, correctly sized struct.
                let mut info: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
                unsafe {
                    GetFileInformationByHandleEx(
                        handle.0,
                        FileBasicInfo,
                        std::ptr::from_mut(&mut info).cast(),
                        u32::try_from(size_of::<FILE_BASIC_INFO>()).expect("a small struct"),
                    ) != 0
                }
            }
            Self::IdInfo => {
                // SAFETY: as above.
                let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
                unsafe {
                    GetFileInformationByHandleEx(
                        handle.0,
                        FileIdInfo,
                        std::ptr::from_mut(&mut info).cast(),
                        u32::try_from(size_of::<FILE_ID_INFO>()).expect("a small struct"),
                    ) != 0
                }
            }
            Self::NonEx => {
                // SAFETY: as above.
                let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
                unsafe { GetFileInformationByHandle(handle.0, &mut info) != 0 }
            }
        }
    }
}

/// The names a directory yields, read start to finish on one handle.
///
/// This is the ground truth every other observation is compared against, and it
/// asserts that the fixture actually needs more than one call -- otherwise every
/// cursor question below would be vacuous.
///
/// # Panics
///
/// Panics if the whole directory fits in a single call.
#[must_use]
pub fn ground_truth(fixture: &Fixture) -> Vec<String> {
    let handle = DirHandle::open(fixture.path());
    let first = handle.enumerate(true).expect("the restart call");
    let rest = handle.drain();
    let all: Vec<String> = first.iter().cloned().chain(rest).collect();
    assert!(
        all.len() > first.len(),
        "vacuous fixture: the whole directory fitted in one call"
    );
    all
}

/// What a duplicate returned when asked to continue an enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorObservation {
    /// Names the source handle read first.
    pub source_first: Vec<String>,
    /// Names the second handle produced afterwards.
    pub other_next: Result<Vec<String>, u32>,
}

impl CursorObservation {
    /// The second handle continued from where the first stopped.
    #[must_use]
    pub fn continued(&self, truth: &[String]) -> bool {
        let Ok(next) = &self.other_next else {
            return false;
        };
        if next.is_empty() {
            return false;
        }
        let start = self.source_first.len();
        truth
            .get(start..start + next.len())
            .is_some_and(|expected| expected == next.as_slice())
    }

    /// The second handle started again from the beginning.
    ///
    /// Only meaningful alongside [`left_mid_directory`]: if the first call had
    /// drained the whole directory, both handles would return the complete
    /// listing and this would be trivially true with no cursor ever left
    /// part-way through.
    ///
    /// [`left_mid_directory`]: Self::left_mid_directory
    #[must_use]
    pub fn restarted(&self) -> bool {
        self.other_next
            .as_ref()
            .is_ok_and(|next| next.as_slice() == self.source_first.as_slice())
    }

    /// The first handle really was left part-way through the directory.
    ///
    /// This is what makes [`restarted`](Self::restarted) and
    /// [`continued`](Self::continued) distinguishable at all. A first call
    /// that returned everything would satisfy `restarted` without any cursor
    /// having been suspended, so the control would report "independent
    /// cursors" having observed nothing of the sort.
    #[must_use]
    pub fn left_mid_directory(&self, truth: &[String]) -> bool {
        !self.source_first.is_empty() && self.source_first.len() < truth.len()
    }
}

/// Does a duplicated handle continue the source's enumeration?
#[must_use]
pub fn duplicate_shares_cursor(fixture: &Fixture) -> CursorObservation {
    let source = DirHandle::open(fixture.path());
    let source_first = source.enumerate(true).expect("the restart call");
    let duplicate = source.duplicate();
    CursorObservation {
        source_first,
        other_next: duplicate.enumerate(false),
    }
}

/// The control: does a *separate open* continue the first handle's enumeration?
///
/// Without this, "the duplicate continued" is unattributable -- it could mean
/// any handle continues, which would say nothing about duplication.
///
/// Check [`CursorObservation::left_mid_directory`] against
/// [`ground_truth`] before reading the result. This probe cannot check it
/// itself without draining the directory, which would disturb the very cursor
/// it is measuring.
#[must_use]
pub fn separate_opens_are_independent(fixture: &Fixture) -> CursorObservation {
    let first = DirHandle::open(fixture.path());
    let source_first = first.enumerate(true).expect("the restart call");
    let second = DirHandle::open(fixture.path());
    CursorObservation {
        source_first,
        other_next: second.enumerate(false),
    }
}

/// Can the source still enumerate after its duplicate is closed?
///
/// A design in which a request owns a duplicate and drops it depends on this
/// answer being yes.
#[must_use]
pub fn closing_duplicate_preserves_source(fixture: &Fixture) -> bool {
    let source = DirHandle::open(fixture.path());
    let _ = source.enumerate(true).expect("the restart call");
    let duplicate = source.duplicate();
    drop(duplicate);
    source.enumerate(false).is_ok()
}

/// Does an interleaved single-shot query move the enumeration cursor?
///
/// `on_duplicate` sends the query to a duplicate of the enumerating handle
/// rather than to the handle itself.
///
/// Returns `(query_succeeded, cursor_was_disturbed)`.
#[must_use]
pub fn query_disturbs_cursor(
    fixture: &Fixture,
    query: SingleShot,
    on_duplicate: bool,
    truth: &[String],
) -> (bool, bool) {
    let handle = DirHandle::open(fixture.path());
    let before = handle.enumerate(true).expect("the restart call");
    let succeeded = if on_duplicate {
        query.run(&handle.duplicate())
    } else {
        query.run(&handle)
    };
    let after = handle.enumerate(false);
    let observation = CursorObservation {
        source_first: before,
        other_next: after,
    };
    (succeeded, !observation.continued(truth))
}
