// Copyright (c) 2026 Mike Grier
//! Helpers shared by this crate's unit tests.

use std::path::{Path, PathBuf};

/// A uniquely named temp directory, removed when the test passes.
///
/// Cleanup is deliberately not RAII: an assertion failure leaves the tree behind
/// for post-mortem inspection, which for a watcher test is often the only record
/// of what the kernel actually saw.
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub(crate) fn new(label: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "windows-file-watcher-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("create temp dir");
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn cleanup(self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
