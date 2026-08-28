// Copyright (c) 2026 Mike Grier
//! Shared support for the integration suite: everything here is built purely
//! on the crate's public API, because an integration test is a separate crate
//! linked against it -- `src/scratch.rs` and `src/testing.rs` are `pub(crate)`
//! and simply do not exist from here.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use windows_file_enumeration_sys::{Completion, EnumerationId, Receiver, TerminalOutcome};

/// How long any single integration scenario is willing to wait for a
/// completion. Generous: these tests run against real directories and a real
/// thread pool, not a scripted clock.
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// Distinguishes fixtures created within the same process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A temporary directory that deletes itself and everything under it.
pub struct Scratch {
    path: PathBuf,
}

impl Scratch {
    /// Create an empty scratch directory.
    #[must_use]
    pub fn empty() -> Self {
        let unique = format!(
            "windows-file-enumeration-sys-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = std::env::temp_dir().join(unique);
        // A previous run that was killed mid-test could have left this behind.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Self { path }
    }

    /// Create a scratch directory holding `names` as empty files.
    #[must_use]
    pub fn with_files(names: &[&str]) -> Self {
        let scratch = Self::empty();
        for name in names {
            std::fs::write(scratch.path().join(name), b"").expect("a scratch file");
        }
        scratch
    }

    /// The directory's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory, which need not exist.
    #[must_use]
    pub fn child(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Create an empty subdirectory inside this one, returning its path.
    pub fn subdir(&self, name: &str) -> PathBuf {
        let path = self.child(name);
        std::fs::create_dir_all(&path).expect("a scratch subdirectory");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Best effort: a leaked temporary directory is not worth failing a
        // test that already made its point, and panicking here would mask it.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// `count` distinct, comparison-friendly file names.
#[must_use]
pub fn many_file_names(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("f{index:06}.dat")).collect()
}

/// Borrow a `Vec<String>` as the `&[&str]` `Scratch::with_files` wants.
#[must_use]
pub fn borrow_all(names: &[String]) -> Vec<&str> {
    names.iter().map(String::as_str).collect()
}

/// Create a directory junction at `link` pointing at `target`, using
/// `mklink` rather than a Win32 call directly: junctions need no privilege a
/// plain user lacks, unlike `CreateSymbolicLinkW`.
pub fn create_junction(link: &Path, target: &Path) {
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("mklink is part of every Windows installation");
    assert!(status.success(), "mklink /J failed for {}", link.display());
}

/// Drain a receiver until `enumeration`'s terminal arrives, collecting every
/// entry delivered for it along the way.
///
/// Panics if no terminal arrives within [`RECV_TIMEOUT`], or if a record for a
/// different enumeration is observed on this receiver. Returns as soon as
/// `enumeration`'s terminal arrives; it relies on the session's own guarantee
/// of exactly one terminal per enumeration rather than continuing to drain
/// afterward to re-verify that guarantee itself.
pub fn drain_to_terminal(
    receiver: &Receiver,
    enumeration: EnumerationId,
) -> (Vec<Completion>, TerminalOutcome) {
    let mut entries = Vec::new();
    loop {
        match receiver.recv_timeout(RECV_TIMEOUT) {
            Some(record @ Completion::Entry { .. }) => {
                assert_eq!(record.enumeration(), enumeration, "a foreign entry arrived");
                entries.push(record);
            }
            Some(Completion::Terminal {
                enumeration: id,
                outcome,
            }) => {
                assert_eq!(id, enumeration, "a foreign terminal arrived");
                return (entries, outcome);
            }
            None => panic!("no terminal arrived for {enumeration} within {RECV_TIMEOUT:?}"),
        }
    }
}

/// The entry names [`drain_to_terminal`] collected, in delivery order.
#[must_use]
pub fn entry_names(entries: &[Completion]) -> Vec<String> {
    entries
        .iter()
        .map(|record| match record {
            Completion::Entry { entry, .. } => entry.name().to_string_lossy(),
            Completion::Terminal { .. } => unreachable!("terminals are never collected as entries"),
        })
        .collect()
}

/// Drain a receiver shared by several concurrently running enumerations until
/// every one of `enumerations` has reported its terminal, returning each
/// enumeration's own entries and outcome.
///
/// Per-enumeration ordering (no entry after that enumeration's own terminal)
/// is still checked; ordering *between* different enumerations' records is
/// not, because concurrently running enumerations interleave arbitrarily.
pub fn drain_many(
    receiver: &Receiver,
    enumerations: &[EnumerationId],
) -> std::collections::HashMap<EnumerationId, (Vec<Completion>, TerminalOutcome)> {
    let mut pending: std::collections::HashSet<EnumerationId> =
        enumerations.iter().copied().collect();
    let mut entries: std::collections::HashMap<EnumerationId, Vec<Completion>> =
        std::collections::HashMap::new();
    let mut finished = std::collections::HashMap::new();

    while !pending.is_empty() {
        match receiver.recv_timeout(RECV_TIMEOUT) {
            Some(record @ Completion::Entry { .. }) => {
                let id = record.enumeration();
                assert!(
                    pending.contains(&id),
                    "an entry arrived for {id} after its own terminal, or for an enumeration \
                     this call was not told to expect"
                );
                entries.entry(id).or_default().push(record);
            }
            Some(Completion::Terminal {
                enumeration,
                outcome,
            }) => {
                assert!(
                    pending.remove(&enumeration),
                    "a terminal arrived twice for {enumeration}, or for an enumeration this \
                     call was not told to expect"
                );
                let collected = entries.remove(&enumeration).unwrap_or_default();
                finished.insert(enumeration, (collected, outcome));
            }
            None => panic!("{} enumeration(s) never reported a terminal", pending.len()),
        }
    }
    finished
}
