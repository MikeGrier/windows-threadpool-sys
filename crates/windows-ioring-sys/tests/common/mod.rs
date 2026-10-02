// Copyright (c) 2026 Mike Grier
//! Shared test helpers.
//!
//! Not a test target: `cargo` compiles only the top-level `.rs` files in
//! `tests/` as integration tests, so a module in a subdirectory is included by
//! the files that declare `mod common;` rather than run on its own.

#![allow(dead_code, reason = "each test target includes only the parts it uses")]

use std::path::{Path, PathBuf};

/// A path under the system temp directory that removes itself when dropped.
///
/// Exists because the explicit `remove_file` at the end of a test body does
/// **not** run when the test panics, and a panicking test is exactly the case
/// that was leaking: the `M26.13` stall investigation left **307,383 files and
/// 1.2 GB** behind over roughly thirty thousand runs. Even an all-passing run
/// of this crate's suite left 25 files, because several paths had no removal at
/// all.
///
/// # Deleting a file whose handle is still open fails
///
/// These tests open their files with `FILE_SHARE_READ | FILE_SHARE_WRITE` and
/// no `FILE_SHARE_DELETE`, so a removal attempted while a handle is open cannot
/// succeed. Struct fields and locals drop *after* the enclosing `Drop` body, and
/// locals drop in reverse declaration order, so a guard declared **before** the
/// handle it shadows is removed after that handle closes.
///
/// That ordering is why an explicit `remove_file` placed after the test closes
/// its own handle is kept where it already exists rather than deleted as
/// redundant: it runs at a point the test controls, and this guard is the net
/// underneath it for the panicking path. A removal that finds nothing is not an
/// error here, so the two compose.
///
/// The hazard is not hypothetical -- see the `Fixture` in
/// [flush_barrier_stress.rs](../flush_barrier_stress.rs), where holding the
/// handle directly meant every trial silently leaked a 32 MiB extent.
pub struct TempPath {
    path: PathBuf,
}

impl TempPath {
    /// A path named for `tag`, unique to this process.
    ///
    /// The process id is part of the name because the reproducer runs the same
    /// test in thousands of fresh processes, and a shared name would have them
    /// racing for one file.
    pub fn new(prefix: &str, tag: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "windows-ioring-sys-{prefix}-{tag}-{}.tmp",
                std::process::id()
            )),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for TempPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for TempPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.path.fmt(f)
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            // The test removed it already, or never created it. Both are
            // ordinary, and neither is worth a line of output.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            // Reported rather than discarded. A silent `let _ =` is what let
            // the 32 MiB-per-trial leak in flush_barrier_stress.rs run for a
            // whole session unnoticed; a warning during teardown costs nothing.
            Err(error) => eprintln!(
                "warning: could not remove the temp file {}: {error}",
                self.path.display()
            ),
        }
    }
}
