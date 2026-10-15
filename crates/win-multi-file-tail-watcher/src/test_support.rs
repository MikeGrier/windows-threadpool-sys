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
