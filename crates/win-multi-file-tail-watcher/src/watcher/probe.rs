// Copyright (c) 2026 Mike Grier
//! The probe pass: read each snapshotted length with no lock held (TW-D-7, TW-D-13).
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the poll engine (TW-3.1) runs it")
)]

use std::io;

use windows_overlapped_io_sys::EndpointObserver;

use super::{SnapshotItem, Watcher};
use crate::chunked::Chunked;
use crate::diagnostics::{Diagnostic, DiagnosticKind};
use crate::probe_failure::{FailureClass, classify, win32_code};

#[cfg(test)]
mod tests;

/// What the probe found for one registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Its length is not the one last reported.
    Changed { current: u64 },
    /// Its handle can never answer again, with the code that said so.
    Terminal { win32_code: u32 },
}

/// A registration the report pass has something to say about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    pub(crate) id: u64,
    pub(crate) last_known: u64,
    pub(crate) outcome: Outcome,
}

impl<T: Clone> Watcher<T> {
    /// Read every snapshotted observer's length, dropping each observer as soon as it has been
    /// read, and append to `changed`, in the snapshot's order, the registrations whose length
    /// differs from the last one reported, and those that failed terminally. `snapshot` is empty
    /// afterwards and keeps its chunks.
    ///
    /// A failure that may not recur is reported to the diagnostic sink and the registration is
    /// probed again next time; nothing is kept for it.
    pub(crate) fn probe(
        &self,
        snapshot: &mut Chunked<SnapshotItem>,
        changed: &mut Chunked<Change>,
    ) {
        self.probe_with(snapshot, changed, EndpointObserver::file_len);
    }

    // `read` is the probe itself, so that a test can make one fail.
    pub(super) fn probe_with(
        &self,
        snapshot: &mut Chunked<SnapshotItem>,
        changed: &mut Chunked<Change>,
        read: impl Fn(&EndpointObserver) -> io::Result<u64>,
    ) {
        snapshot.drain_each(|item| {
            let SnapshotItem {
                id,
                last_known,
                observer,
            } = item;
            match read(&observer) {
                Ok(current) if current != last_known => changed.push(Change {
                    id,
                    last_known,
                    outcome: Outcome::Changed { current },
                }),
                Ok(_) => {}
                Err(error) => match (classify(&error), win32_code(&error)) {
                    (FailureClass::Terminal, Some(code)) => changed.push(Change {
                        id,
                        last_known,
                        outcome: Outcome::Terminal { win32_code: code },
                    }),
                    _ => self.diagnose(DiagnosticKind::ProbeFailed, || Diagnostic::ProbeFailed {
                        registration: id,
                        win32_code: win32_code(&error),
                    }),
                },
            }
        });
    }
}
