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
//! # What a quantum does today
//!
//! Open the directory, obtain any volume identity the request asked for, and
//! read the first batch. An empty directory finishes here; so does a failure.
//! A directory that *has* entries is left waiting for the record parser (FE-9),
//! which is what turns a batch into entries.

use std::os::windows::io::OwnedHandle;

use windows_impersonation_token_sys::ImpersonationToken;

use crate::buffer::NativeBuffer;
use crate::completion::TerminalOutcome;
use crate::entry::FileIdentityMode;
use crate::error::EnumerationError;
use crate::native::{self, Refill, RefillOutcome};
use crate::request::EnumerationRequest;
use crate::session::QuantumOutcome;

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
        }
    }

    /// The volume serial, once one has been obtained.
    ///
    /// `None` both when the request did not ask for one and when a best-effort
    /// query failed, which is deliberate: an identity is either volume-qualified
    /// or it is not, and a caller cannot act on the difference between those two
    /// reasons.
    #[allow(dead_code, reason = "FE-9 stamps this onto every entry it builds")]
    pub(crate) fn volume_serial(&self) -> Option<u64> {
        self.volume_serial
    }

    /// The request being served.
    #[allow(dead_code, reason = "FE-9 evaluates this request's predicate")]
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
}

impl std::fmt::Debug for EngineState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineState")
            .field("phase", &self.phase)
            .field("open", &self.directory.is_some())
            .field("volume_serial", &self.volume_serial)
            .finish_non_exhaustive()
    }
}

/// Advance one enumeration by one quantum.
///
/// Runs with no lock held, so it is free to block on a directory query.
pub(crate) fn advance(engine: &mut EngineState) -> QuantumOutcome {
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

    let which = match engine.phase {
        Phase::Opened => Refill::First,
        _ => Refill::Next,
    };
    match native::refill(directory, &mut engine.buffer, which) {
        RefillOutcome::Batch => {
            engine.phase = Phase::Reading;
            // One refill per quantum, then hand the worker back: that puts a
            // scheduling point at every place this could have blocked.
            //
            // FE-9 is what turns this batch into entries. Until it lands the
            // batch is read and passed over, so an enumeration still reaches
            // its true end -- it simply delivers nothing on the way.
            QuantumOutcome::Yielded
        }
        RefillOutcome::Exhausted => QuantumOutcome::Finished(TerminalOutcome::Completed),
        RefillOutcome::Failed(error) => QuantumOutcome::Finished(TerminalOutcome::Failed(error)),
    }
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
