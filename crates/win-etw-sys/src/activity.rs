// Copyright (c) 2026 Mike Grier
//! `EventActivityIdControl` with `EVENT_ACTIVITY_CTRL_CREATE_ID`: a new activity id (WE-D-5).
//!
//! Read 2026-10-21 from the call's page (`ms.date` 2018-12-05). The code "Sets the ActivityId parameter to the value of a
//! newly-generated locally-unique activity ID", and the remarks say the call "generates locally-unique IDs that are
//! guaranteed to be unique across all processes on the local system until the system reboots". The call's other codes get
//! and set the calling thread's activity id, which a writer that names its activities on every write does not need, and they
//! are not wrapped.

use win_error_codes::Win32Error;
use windows_sys::Win32::System::Diagnostics::Etw::{
    EVENT_ACTIVITY_CTRL_CREATE_ID, EventActivityIdControl,
};
use windows_sys::core::GUID;

use crate::guid::Guid;

#[cfg(test)]
mod tests;

/// A newly-generated activity id, unique across every process of this system until it restarts, as the page guarantees.
/// The calling thread's own activity id is left as it was.
///
/// # Errors
///
/// The call's failure, with its code. The page names only success, `ERROR_SUCCESS`, and lists no failure.
pub fn create_activity_id() -> Result<Guid, Win32Error> {
    let mut id = GUID {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };
    // SAFETY: the code is one the page lists, and `id` is a valid GUID the call writes the new id into.
    let code = unsafe { EventActivityIdControl(EVENT_ACTIVITY_CTRL_CREATE_ID, &raw mut id) };
    Win32Error::from_code(code).into_result()?;
    Ok(Guid::from_raw(id))
}
