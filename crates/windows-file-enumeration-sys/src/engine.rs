// Copyright (c) 2026 Mike Grier
//! One enumeration's native state, and the quantum that advances it.
//!
//! # Why this leaves the registry while it runs
//!
//! A refill is a synchronous directory query that may block for as long as a
//! network round trip. Holding the registry lock across it would stall every
//! begin, cancel, and retirement in the session behind one slow volume. So a
//! worker *takes* this state out of its registry entry when it claims the
//! enumeration, runs the whole quantum with no lock held, and hands it back when
//! it reports.
//!
//! Single-flight claiming is what makes that sound: exactly one worker holds
//! this at a time, so the directory handle, the buffer, and the record cursor
//! have one owner for the duration.
//!
//! # What a quantum does
//!
//! Open the directory and obtain any volume identity the request asked for, on
//! the quantum that finds it unopened. A refill happens at most once per
//! quantum: when the record cursor says the loaded batch is exhausted, one
//! `GetFileInformationByHandleEx` call loads the next one. Whatever batch is
//! then current -- freshly loaded or left over from a quantum that could not
//! finish it -- is parsed record by record: `.` and `..` are dropped, the
//! request's predicate is evaluated, and every match is offered to the
//! completion ring. The cursor this leaves behind, in
//! [`EngineState::cursor`], is exactly where the next quantum resumes; a batch
//! is never re-read from its start and never refilled a second time before it
//! is drained.
//!
//! Parsing stops early, before the batch is drained, for three reasons, and
//! each leaves the cursor at a different place:
//!
//! - The completion ring refuses an accepted entry. The cursor is left at
//!   that record -- not past it -- so a later quantum reparses and re-offers
//!   exactly what could not be delivered, rather than losing it.
//!   [`EngineState::awaiting_room`] remembers that the record waiting there is
//!   already known to need delivery, so a quantum that resumes while the ring
//!   is still full can say so from one cheap check rather than reparsing,
//!   rebuilding, and re-evaluating a predicate against a record whose fate is
//!   already decided.
//! - The quantum's record budget or time budget is spent
//!   ([`quantum_budget_exhausted`]). The cursor is left at the next unexamined
//!   record, and the quantum yields rather than parking: nothing is blocked on
//!   the completion ring, only on getting the worker back. This is what keeps
//!   an enormous batch, or a predicate that rejects every record it sees,
//!   from monopolising a worker.
//! - A record fails validation, which ends the enumeration outright rather
//!   than leaving a cursor to resume from.

use std::os::windows::io::OwnedHandle;
use std::time::{Duration, Instant};

use windows_impersonation_token_sys::ImpersonationToken;

use crate::buffer::NativeBuffer;
use crate::completion::{Completion, EnumerationId, TerminalOutcome};
use crate::completion_ring::CompletionRing;
use crate::entry::{DirectoryEntry, FileIdentityMode};
use crate::error::EnumerationError;
use crate::native::{self, Refill, RefillOutcome};
use crate::record;
use crate::request::EnumerationRequest;
use crate::session::QuantumOutcome;

/// The most records one quantum examines before yielding, regardless of how
/// each one was disposed of.
///
/// Counted against every record a quantum looks at -- a dropped `.` or `..`,
/// one a predicate rejected, and one delivered all count the same -- so a
/// directory whose predicate matches nothing still yields back to the
/// scheduler instead of running to the end of an enormous batch in one
/// callback.
const MAX_RECORDS_PER_QUANTUM: u32 = 256;

/// The longest one quantum may run before yielding, regardless of how many
/// records that turned out to be.
///
/// A record count alone is too coarse when per-record cost varies -- a long
/// name, or an expensive predicate clause -- so this is the second, orthogonal
/// bound: whichever budget is spent first ends the quantum.
const MAX_QUANTUM_DURATION: Duration = Duration::from_millis(2);

/// Whether a quantum that has already examined `examined` records over
/// `elapsed` time should stop before examining another.
///
/// Never true for a quantum's first record: `examined` starts at zero, so a
/// quantum always makes at least one record's worth of progress no matter how
/// tight either bound is -- a budget that could stall an enumeration
/// completely is not a budget.
fn quantum_budget_exhausted(examined: u32, elapsed: Duration) -> bool {
    examined > 0 && (examined >= MAX_RECORDS_PER_QUANTUM || elapsed >= MAX_QUANTUM_DURATION)
}

/// How far an enumeration has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Nothing has been opened yet.
    Unopened,
    /// The directory is open and the next refill is the first one.
    Opened,
    /// At least one batch has been read, so a refill is no longer the first.
    Reading,
}

/// Everything one enumeration needs to make progress.
pub(crate) struct EngineState {
    request: EnumerationRequest,
    token: ImpersonationToken,
    buffer: NativeBuffer,
    directory: Option<OwnedHandle>,
    volume_serial: Option<u64>,
    phase: Phase,
    /// Where the next record starts in the currently loaded batch.
    ///
    /// `None` means there is no unparsed batch left: either nothing has been
    /// read yet, or the last one was fully drained, and the next quantum's
    /// first job is a refill rather than a parse.
    cursor: Option<usize>,
    /// Whether the record at `cursor` is already known to need delivery and
    /// was refused for want of completion-ring room.
    ///
    /// Set only when a quantum parks; cleared the moment a resuming quantum
    /// finds room. While set, resuming checks the ring before reparsing and
    /// rebuilding a record whose fate -- deliver, once there is room -- is
    /// already decided.
    awaiting_room: bool,
}

impl EngineState {
    pub(crate) fn new(
        request: EnumerationRequest,
        token: ImpersonationToken,
        buffer: NativeBuffer,
    ) -> Self {
        Self {
            request,
            token,
            buffer,
            directory: None,
            volume_serial: None,
            phase: Phase::Unopened,
            cursor: None,
            awaiting_room: false,
        }
    }

    /// The volume serial, once one has been obtained.
    ///
    /// `None` both when the request did not ask for one and when a best-effort
    /// query failed, which is deliberate: an identity is either volume-qualified
    /// or it is not, and a caller cannot act on the difference between those two
    /// reasons.
    pub(crate) fn volume_serial(&self) -> Option<u64> {
        self.volume_serial
    }

    /// The request being served.
    pub(crate) fn request(&self) -> &EnumerationRequest {
        &self.request
    }

    /// Give the caller's request and captured context back.
    ///
    /// Used when a begin is refused after this state was built: nothing was
    /// accepted, so the caller gets to retry with exactly what it submitted.
    pub(crate) fn into_parts(self) -> (EnumerationRequest, ImpersonationToken) {
        (self.request, self.token)
    }

    /// Whether a batch is loaded and not yet fully parsed.
    #[cfg(test)]
    pub(crate) fn has_pending_batch(&self) -> bool {
        self.cursor.is_some()
    }
}

impl std::fmt::Debug for EngineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineState")
            .field("phase", &self.phase)
            .field("open", &self.directory.is_some())
            .field("volume_serial", &self.volume_serial)
            .field("cursor", &self.cursor)
            .field("awaiting_room", &self.awaiting_room)
            .finish_non_exhaustive()
    }
}

/// Advance one enumeration by one quantum.
///
/// Runs with no lock held, so it is free to block on a directory query.
pub(crate) fn advance(
    engine: &mut EngineState,
    enumeration: EnumerationId,
    completions: &CompletionRing,
) -> QuantumOutcome {
    if engine.phase == Phase::Unopened
        && let Some(failure) = start(engine)
    {
        return QuantumOutcome::Finished(TerminalOutcome::Failed(failure));
    }

    let Some(directory) = engine.directory.as_ref() else {
        // Unreachable while `start` either opens or fails, but reporting a
        // failure beats an unwrap in a thread-pool callback.
        return QuantumOutcome::Finished(TerminalOutcome::Failed(
            EnumerationError::DirectoryQuery(crate::error::Win32Error::from_code(0)),
        ));
    };

    if engine.cursor.is_none() {
        let which = match engine.phase {
            Phase::Opened => Refill::First,
            _ => Refill::Next,
        };
        match native::refill(directory, &mut engine.buffer, which) {
            RefillOutcome::Batch => {
                engine.phase = Phase::Reading;
                engine.cursor = Some(0);
            }
            RefillOutcome::Exhausted => {
                return QuantumOutcome::Finished(TerminalOutcome::Completed);
            }
            RefillOutcome::Failed(error) => {
                return QuantumOutcome::Finished(TerminalOutcome::Failed(error));
            }
        }
    }

    // Parse as much of the current batch as this quantum's budgets allow. One
    // refill is already spent above, so draining this batch never triggers
    // another -- that is what leaves a scheduling point at every place a
    // refill could have blocked.
    let started = Instant::now();
    let mut examined: u32 = 0;

    while let Some(offset) = engine.cursor {
        if engine.awaiting_room {
            if !completions.has_data_room() {
                return QuantumOutcome::Parked;
            }
            engine.awaiting_room = false;
        }

        if quantum_budget_exhausted(examined, started.elapsed()) {
            return QuantumOutcome::Yielded;
        }

        let (parsed, next) = match record::parse_record(engine.buffer.as_bytes(), offset) {
            Ok(parsed) => parsed,
            Err(detail) => {
                return QuantumOutcome::Finished(TerminalOutcome::Failed(
                    EnumerationError::MalformedRecord(detail),
                ));
            }
        };
        examined += 1;

        if parsed.is_dot_or_dotdot() {
            engine.cursor = next;
            continue;
        }

        let entry = DirectoryEntry::from_fields(parsed.into_fields(engine.volume_serial()));
        if !engine.request().predicate().matches(&entry) {
            engine.cursor = next;
            continue;
        }

        match completions.try_send_entry(Completion::Entry { enumeration, entry }) {
            Ok(()) => engine.cursor = next,
            Err(_) => {
                // No room. The cursor stays exactly here -- not past it -- so
                // the next quantum reparses and re-offers this same record
                // rather than losing it, and knows to check for room first.
                engine.awaiting_room = true;
                return QuantumOutcome::Parked;
            }
        }
    }

    QuantumOutcome::Yielded
}

/// Open the directory and obtain whatever identity the request asked for.
///
/// Returns the failure that ends the enumeration, or `None` when it is ready to
/// read.
fn start(engine: &mut EngineState) -> Option<EnumerationError> {
    let directory = match native::open_directory(engine.request.path(), &engine.token) {
        Ok(directory) => directory,
        Err(error) => return Some(error),
    };

    let mode = engine.request.file_identity_mode();
    if mode.queries_volume() {
        match native::volume_serial(&directory) {
            Ok(serial) => engine.volume_serial = Some(serial),
            Err(code) => {
                // `Required` means an unqualified identity would be silently
                // wrong, so the enumeration fails before its first entry rather
                // than delivering identities that cannot be compared.
                if mode == FileIdentityMode::Required {
                    return Some(EnumerationError::VolumeIdentity(code));
                }
            }
        }
    }

    engine.directory = Some(directory);
    engine.phase = Phase::Opened;
    None
}

#[cfg(test)]
mod tests;
