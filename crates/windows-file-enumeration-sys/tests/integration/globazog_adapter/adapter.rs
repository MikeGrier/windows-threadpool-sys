// Copyright (c) 2026 Mike Grier
//! The adapter itself: `enumerate_dir_native_via_wfe` has the same signature
//! and the same observable contract as Globazog's
//! `crates/globazog/src/sys/win.rs::enumerate_dir_native`, built entirely on
//! `windows_file_enumeration_sys`'s public `Session` / `Receiver` API.
//!
//! # No per-entry opens
//!
//! Every field this adapter fills on a [`DirEntry`] comes from the
//! `Completion::Entry` record the session already delivered -- name, type,
//! reparse status and tag, raw attributes, size, all four times, and file
//! identity are all inline in that one record. This function never calls
//! anything that opens an individual entry; the only handle it opens at all
//! is the one directory handle `Session::try_begin` opens once per
//! enumeration, which is D-3's own guarantee, inherited rather than
//! reproved here. `tests_no_per_entry_open.rs` adds an empirical check
//! specific to this adapter: a directory junction whose target does not
//! exist is still reported with full metadata, which a per-entry open of
//! the junction would have failed.

use std::io;
use std::path::Path;
use std::time::Duration;

use windows_file_enumeration_sys::{
    Completion, EntryPredicate, EnumerationRequest, FileIdentityMode, Session, TerminalOutcome,
};

use crate::globazog_adapter::predicate_types::Leaf;
use crate::globazog_adapter::translate::translate_leaves;
use crate::globazog_adapter::types::{DirEntry, DirScan, EntryFailure, EnumPlan, FileId};

/// How long this adapter waits for one completion before treating the
/// enumeration as hung. Generous: these tests run against real directories
/// and the real thread pool.
const RECV_TIMEOUT: Duration = Duration::from_secs(60);

/// Enumerate one directory exactly as Globazog's native Windows backend
/// would, with no predicate applied.
///
/// # Errors
///
/// Returns `Err` when the directory could not be opened, or when a query
/// failed before any entry was read -- mirroring Globazog's own contract
/// that a root which yields no usable listing at all is a fatal error, never
/// a `DirScan`.
pub fn enumerate_dir_native_via_wfe(path: &Path, plan: EnumPlan) -> io::Result<DirScan> {
    enumerate_dir_native_via_wfe_with_predicate(path, plan, &[])
}

/// As [`enumerate_dir_native_via_wfe`], additionally applying `leaves` as a
/// translated predicate -- the shape a caller composing Globazog's own
/// predicate leaves with a one-directory backend would need.
///
/// # Errors
///
/// As [`enumerate_dir_native_via_wfe`].
pub fn enumerate_dir_native_via_wfe_with_predicate(
    path: &Path,
    plan: EnumPlan,
    leaves: &[Leaf],
) -> io::Result<DirScan> {
    let identity_mode = if plan.wants_any_file_id() {
        FileIdentityMode::BestEffort
    } else {
        FileIdentityMode::Omit
    };

    let mut request = EnumerationRequest::for_path(path)
        .map_err(|error| io::Error::other(error.to_string()))?
        .with_file_identity(identity_mode);
    if !leaves.is_empty() {
        request = request.with_predicate(EntryPredicate::from(translate_leaves(leaves)));
    }

    // One session per call, matching Globazog's own `enumerate_dir_native`:
    // it is a synchronous, one-shot function with no state to share across
    // calls. Capacities generous enough that no ordinary directory this
    // adapter's tests use ever parks on ring room; that backpressure
    // behavior is already proven directly against the engine in FE-10.
    let (session, receiver) =
        Session::new(64, 4096).map_err(|error| io::Error::other(error.to_string()))?;
    let handle = session
        .try_begin(request)
        .map_err(|error| io::Error::other(error.to_string()))?;
    handle.detach();

    let mut entries = Vec::new();
    let outcome = loop {
        match receiver.recv_timeout(RECV_TIMEOUT) {
            Some(Completion::Entry { entry, .. }) => {
                entries.push(translate_entry(&entry, &plan));
            }
            Some(Completion::Terminal { outcome, .. }) => break outcome,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no terminal arrived within the adapter's receive timeout",
                ));
            }
        }
    };
    finish_scan(entries, outcome)
}

/// Translate one native entry into Globazog's `DirEntry` shape.
fn translate_entry(
    entry: &windows_file_enumeration_sys::DirectoryEntry,
    plan: &EnumPlan,
) -> DirEntry {
    let entry_type = match entry.entry_type() {
        windows_file_enumeration_sys::EntryType::File => {
            crate::globazog_adapter::types::EntryType::File
        }
        windows_file_enumeration_sys::EntryType::Directory => {
            crate::globazog_adapter::types::EntryType::Dir
        }
    };
    let is_reparse = entry.is_reparse_point();
    let identity = entry.identity();
    let file_id = match identity.volume_serial() {
        Some(volume) if plan.wants_file_id_for(is_reparse) => FileId {
            volume,
            // Little-endian, exactly as Globazog's own `file_id_128` reads
            // the same opaque 16-byte identifier: it is never treated as a
            // meaningful numeric value on either side, only as an equality
            // and hash key, so the byte order only needs to be consistent,
            // not canonical.
            id: u128::from_le_bytes(identity.id_bytes()),
        },
        _ => FileId { volume: 0, id: 0 },
    };

    DirEntry {
        name: crate::globazog_adapter::types::decode_utf16(entry.name().as_units()),
        entry_type,
        is_reparse,
        reparse_tag: entry.reparse_tag().unwrap_or(0),
        attributes: entry.attributes(),
        size: entry.logical_size(),
        btime: crate::globazog_adapter::types::filetime_to_unix_nanos(
            entry.creation_time().ticks(),
        ),
        mtime: crate::globazog_adapter::types::filetime_to_unix_nanos(
            entry.last_write_time().ticks(),
        ),
        atime: crate::globazog_adapter::types::filetime_to_unix_nanos(
            entry.last_access_time().ticks(),
        ),
        ctime: crate::globazog_adapter::types::filetime_to_unix_nanos(entry.change_time().ticks()),
        file_id,
    }
}

/// Turn a drained entry list and a terminal outcome into Globazog's
/// `io::Result<DirScan>` shape.
///
/// Deliberately a pure function taking already-collected values rather than
/// draining a receiver itself: this is what lets
/// `tests_errors.rs` prove the translation -- `Failed` with no entries
/// becomes `Err`, `Failed` with some entries becomes `Ok` plus one
/// `EntryFailure` -- without needing a live enumeration to fail partway
/// through, which nothing in this adapter's public surface can force to
/// happen deterministically.
///
/// # Errors
///
/// Returns `Err` when `outcome` is [`TerminalOutcome::Failed`] and `entries`
/// is empty -- no usable listing was ever produced -- or when `outcome` is
/// [`TerminalOutcome::Cancelled`], which this adapter's own calls never
/// request but which is handled rather than left to panic if a future
/// change ever made it reachable.
pub fn finish_scan(entries: Vec<DirEntry>, outcome: TerminalOutcome) -> io::Result<DirScan> {
    match outcome {
        TerminalOutcome::Completed => Ok(DirScan {
            entries,
            entry_errors: Vec::new(),
        }),
        TerminalOutcome::Cancelled => Err(io::Error::other(
            "the enumeration was cancelled, which this adapter never requests",
        )),
        TerminalOutcome::Failed(error) => {
            let io_error = io::Error::other(error.to_string());
            if entries.is_empty() {
                Err(io_error)
            } else {
                Ok(DirScan {
                    entries,
                    entry_errors: vec![EntryFailure {
                        name: None,
                        source: io_error,
                    }],
                })
            }
        }
    }
}
