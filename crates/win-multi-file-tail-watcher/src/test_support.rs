// Copyright (c) 2026 Mike Grier
//! Helpers the unit tests share.

use std::path::{Path, PathBuf};

use wtf_string::Wtf16String;

/// A fresh directory for one test, removed with its contents when dropped.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "win-multi-file-tail-watcher-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).expect("create the test directory");
        Self(dir)
    }

    pub(crate) fn file(&self, name: impl AsRef<std::ffi::OsStr>, content: &[u8]) -> PathBuf {
        let path = self.0.join(name.as_ref());
        std::fs::write(&path, content).expect("create the test file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn wtf(path: &Path) -> Wtf16String {
    Wtf16String::from(path.as_os_str())
}

pub(crate) fn code(error: &std::io::Error) -> Option<u32> {
    error.raw_os_error().map(i32::cast_unsigned)
}

/// Whether `handle` is signalled within `millis` milliseconds.
pub(crate) fn signalled_within(
    handle: std::os::windows::io::BorrowedHandle<'_>,
    millis: u32,
) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    // SAFETY: a live handle borrowed for the duration of the call; waiting has no other
    // precondition.
    unsafe { WaitForSingleObject(handle.as_raw_handle(), millis) == WAIT_OBJECT_0 }
}

/// Whether `handle` is signalled now.
pub(crate) fn is_signalled(handle: std::os::windows::io::BorrowedHandle<'_>) -> bool {
    signalled_within(handle, 0)
}

/// A sink that keeps each event it is given, and each kind it was asked about.
#[derive(Default)]
pub(crate) struct RecordingSink {
    pub(crate) wanted: bool,
    seen: std::sync::Mutex<Vec<crate::Diagnostic>>,
    asked: std::sync::Mutex<Vec<crate::DiagnosticKind>>,
}

impl RecordingSink {
    pub(crate) fn wanting() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            wanted: true,
            ..Self::default()
        })
    }

    pub(crate) fn unwilling() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self::default())
    }

    pub(crate) fn seen(&self) -> Vec<crate::Diagnostic> {
        self.seen.lock().expect("the record").clone()
    }

    pub(crate) fn asked(&self) -> Vec<crate::DiagnosticKind> {
        self.asked.lock().expect("the record").clone()
    }
}

impl crate::DiagnosticSink for RecordingSink {
    fn wants(&self, kind: crate::DiagnosticKind) -> bool {
        self.asked.lock().expect("the record").push(kind);
        self.wanted
    }

    fn report(&self, event: &crate::Diagnostic) {
        self.seen.lock().expect("the record").push(*event);
    }
}
