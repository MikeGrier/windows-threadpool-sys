// Copyright (c) 2026 Mike Grier
//! Test-only scratch directories.
//!
//! The native layer's subject is real directories, so its tests need real
//! directories: an empty one, one with entries, and a plain file. Each fixture
//! owns a uniquely named directory under the system temporary directory and
//! removes it on drop, so a failing test leaves nothing behind and concurrent
//! tests never collide.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes fixtures created within the same process and millisecond.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary directory that deletes itself.
pub(crate) struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Create an empty scratch directory.
    pub(crate) fn empty() -> Self {
        let unique = format!(
            "windows-file-enumeration-sys-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        // A previous run that was killed mid-test could have left this behind.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).expect("a scratch directory");
        Self { path }
    }

    /// Create a scratch directory holding `names` as empty files.
    pub(crate) fn with_files(names: &[&str]) -> Self {
        let scratch = Self::empty();
        for name in names {
            std::fs::write(scratch.path().join(name), b"").expect("a scratch file");
        }
        scratch
    }

    /// The directory's path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory, which need not exist.
    pub(crate) fn child(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leaked temporary directory is not worth failing a test
        // that has already made its point, and panicking here would mask it.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
