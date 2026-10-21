// Copyright (c) 2026 Mike Grier
//! `EventRegister`, `EventUnregister`, `EventEnabled`, `EventProviderEnabled` and `EventWrite`: a provider's
//! registration, and what is done through it.
//!
//! Read 2026-10-20 from each call's page (all `ms.date` 2018-12-05, `updated_at` 2024-02-22). The registration is
//! "a process-scope operation (the handle is valid only within the process)", it must be undone "before your
//! provider unloads or exits", and "It is not necessary to install a manifest to write events or capture traces",
//! which is the manifest's part in decoding and not in recording. So a registration here is a value that owns the
//! handle and gives it back when dropped.

use std::fmt;

use win_error_codes::Win32Error;
use windows_sys::Win32::System::Diagnostics::Etw::{
    EventEnabled, EventProviderEnabled, EventRegister, EventUnregister, EventWrite,
};

use crate::descriptor::EventDescriptor;
use crate::field::{Field, build};
use crate::guid::Guid;

/// A registered provider: the handle `EventRegister` made, unregistered when this is dropped.
///
/// The handle is valid in this process only. Registering the same GUID twice makes two registrations, each with its own
/// handle, as the page allows (a process may register up to 1,024 providers, "you should limit the number of providers
/// that your component registers to one or two").
#[derive(Debug)]
pub struct Registration {
    /// Never zero for a value that has not been unregistered: `EventRegister` sets zero only on failure, and a failure
    /// is not a value of this type.
    handle: i64,
}

impl Registration {
    /// Registers the provider `provider`, by `EventRegister`, with no enable callback.
    ///
    /// The page: "GUID that uniquely identifies the provider, sometimes called a control GUID. This must be a stable
    /// identifier so that trace controllers can use the GUID to subscribe to events from this provider."
    ///
    /// # Errors
    ///
    /// The call's failure, as the code it returns. The page says that code "is primarily intended for use in debugging
    /// and diagnostic scenarios" and that "Most production code should continue to run even if an ETW provider failed
    /// to register": a caller that does not want the events handles the error by writing none.
    pub fn register(provider: Guid) -> Result<Self, Win32Error> {
        let mut handle: i64 = 0;
        // SAFETY: the provider id is a valid GUID for the call to read, the callback and its context are null, which
        // the page allows, and the handle is a valid `i64` for the call to write.
        let code = unsafe { EventRegister(provider.raw(), None, std::ptr::null(), &mut handle) };
        Win32Error::from_code(code).into_result()?;
        Ok(Self { handle })
    }

    /// Unregisters the provider, by `EventUnregister`, and reports what the call returned.
    ///
    /// A registration that is dropped unregisters just as this does, and drops what the call returned, which the page
    /// says is `ERROR_SUCCESS` when it succeeds and gives no failure for.
    ///
    /// # Errors
    ///
    /// The call's return, if it was not `ERROR_SUCCESS`. The registration is gone either way.
    pub fn unregister(mut self) -> Result<(), Win32Error> {
        let handle = std::mem::take(&mut self.handle);
        Win32Error::from_code(unregister(handle)).into_result()
    }

    /// Whether a session is listening for this event, by `EventEnabled`: `false` "if ETW can quickly determine that no
    /// event collection session would record an event from the given provider with the specified descriptor".
    ///
    /// The page says the test is "conservative": `true` can be returned where "subsequent in-depth filtering would
    /// determine that no sessions need to record the event". [`write`](Self::write) makes this test itself and returns
    /// at once if the event is not enabled, so a caller asks it to avoid the work of preparing the data.
    #[must_use]
    pub fn is_enabled(&self, descriptor: &EventDescriptor) -> bool {
        let raw = descriptor.raw();
        // SAFETY: the handle is one `EventRegister` returned and has not been unregistered, and `raw` is a valid
        // descriptor for the call to read.
        unsafe { EventEnabled(self.handle, &raw) }
    }

    /// As [`is_enabled`](Self::is_enabled), for a caller that has only an event's level and keyword, by
    /// `EventProviderEnabled`. The page: "When a provider has access only to the event's Level and Keyword, the
    /// provider should use" this.
    #[must_use]
    pub fn is_level_enabled(&self, level: u8, keyword: u64) -> bool {
        // SAFETY: the handle is one `EventRegister` returned and has not been unregistered.
        unsafe { EventProviderEnabled(self.handle, level, keyword) }
    }

    /// Writes an event whose data is `fields` in order, by `EventWrite`, with the current thread's activity id.
    ///
    /// "If no trace sessions are recording this event, this function will do nothing and return **ERROR_SUCCESS**."
    /// The fields are written with no padding, as the page says the blocks are.
    ///
    /// # Errors
    ///
    /// [`WriteError::TooManyBlocks`] or [`WriteError::BlockTooLong`] if the fields cannot be given to the call, without
    /// calling it. Otherwise [`WriteError::Call`], the call's failure, which the page lists as `ERROR_INVALID_PARAMETER`
    /// among others, `ERROR_ARITHMETIC_OVERFLOW` for "larger than the allowed maximum (64KB - header)", and
    /// `ERROR_NOT_ENOUGH_MEMORY` when the sessions' buffers cannot keep up. The page says the code "is primarily intended
    /// for use in debugging and diagnostic scenarios" and that most callers ignore it.
    ///
    /// The page gives the number of data descriptors, the call's `UserDataCount`, as "The maximum number is 128". A
    /// terminated string is two descriptors. This function passes however many there are, and the call reports what it
    /// makes of that.
    pub fn write(
        &self,
        descriptor: &EventDescriptor,
        fields: &[Field<'_>],
    ) -> Result<(), WriteError> {
        let built = build(fields)?;
        let (count, blocks) = built.as_call();
        let raw = descriptor.raw();
        // SAFETY: the handle is one `EventRegister` returned and has not been unregistered; `raw` is a valid descriptor;
        // `blocks` points at `count` descriptors that `built` owns, each of which describes memory that the borrowed
        // `fields` hold or a static, all valid for reads of the size given, and `built` and `fields` outlive the call.
        let code = unsafe { EventWrite(self.handle, &raw, count, blocks) };
        Win32Error::from_code(code)
            .into_result()
            .map_err(WriteError::Call)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        unregister(self.handle);
    }
}

/// The call, which does nothing for the zero handle a registration that has been unregistered holds.
fn unregister(handle: i64) -> u32 {
    if handle == 0 {
        return 0;
    }
    // SAFETY: the handle is one `EventRegister` returned and is unregistered once: the caller takes it out of its
    // registration, or the registration is being dropped.
    unsafe { EventUnregister(handle) }
}

/// Why an event was not written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WriteError {
    /// The fields are more blocks than the call's count, a `ULONG`, can hold, so the call was not made. This is the
    /// crate's refusal and not the call's, which would have refused far fewer.
    TooManyBlocks {
        /// How many blocks the fields come to.
        count: usize,
    },
    /// A field's block is longer than a descriptor's size, a `ULONG`, can hold, so the call was not made. This is the
    /// crate's refusal and not the call's, which would have refused far less.
    BlockTooLong {
        /// The place of the field in the list given.
        field: usize,
        /// The block's length in bytes.
        len: usize,
    },
    /// The call failed and gave this code.
    Call(Win32Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyBlocks { count } => write!(
                f,
                "the fields are {count} blocks, more than EventWrite's count, a ULONG, can hold"
            ),
            Self::BlockTooLong { field, len } => write!(
                f,
                "field {field} is {len} bytes, more than an EVENT_DATA_DESCRIPTOR's size, a ULONG, can hold"
            ),
            Self::Call(error) => write!(f, "EventWrite failed: {error}"),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Call(error) => Some(error),
            Self::TooManyBlocks { .. } | Self::BlockTooLong { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;
