// Copyright (c) 2026 Mike Grier
//! The diagnostic sink that writes each event to the provider (DE-D-4).

use std::sync::{Arc, Mutex, PoisonError, Weak};

use durable_ioring::{Diagnostic, DiagnosticKind, DiagnosticSink, Edge};
use win_error_codes::Win32Error;
use win_etw_sys::{Registration, create_activity_id};

use crate::activity;

use crate::encode::{encode, schema_of};
use crate::schema::{PROVIDER_ID, schema_of_kind};

#[cfg(test)]
mod tests;

/// The registration every sink of the process shares while any of them is alive: the provider is registered once, as the page
/// asks ("you should limit the number of providers that your component registers to one or two"), and unregistered when the
/// last sink is dropped, so a library that is unloaded gives it back.
static SHARED: Mutex<Weak<Registration>> = Mutex::new(Weak::new());

fn shared() -> Result<Arc<Registration>, Win32Error> {
    let mut slot = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(registration) = slot.upgrade() {
        return Ok(registration);
    }
    let registration = Arc::new(Registration::register(PROVIDER_ID)?);
    *slot = Arc::downgrade(&registration);
    Ok(registration)
}

/// A sink that writes each event to the provider `durable-ioring-events`, for a session that is listening.
///
/// It records whether or not the provider's manifest is installed: the manifest is what decodes an event for a reader, and
/// installing it is a step for whoever reads the trace and not for the application that writes it. See [`manifest`].
///
/// It wants a kind only when a session is listening for it, so with none its cost is a test of that, and no event is built:
/// no text is formatted and nothing is written. Cloning it shares the registration, which every sink of the process does.
///
/// A pair's begin and end are written in one activity: an id `EventActivityIdControl` creates when the begin is written and
/// the end repeats, whatever thread writes either, and which the platform guarantees no process of the system repeats until it
/// restarts. An end whose begin was not written, because no session was listening then, is written in an activity of its own.
/// Any other event is written in the writing thread's activity. See DE-D-6.
///
/// [`manifest`]: fn@crate::manifest
#[derive(Clone, Debug)]
pub struct EtwDiagnostics {
    registration: Option<Arc<Registration>>,
}

impl EtwDiagnostics {
    /// A sink, registering the provider if no sink of the process has.
    ///
    /// # Errors
    ///
    /// The registration's failure, as the code the call returned. Most callers have no use for it: the page says "Most
    /// production code should continue to run even if an ETW provider failed to register", and [`new`](Self::new) is the sink
    /// that does.
    pub fn try_new() -> Result<Self, Win32Error> {
        Ok(Self {
            registration: Some(shared()?),
        })
    }

    /// A sink, registering the provider if no sink of the process has. If the provider cannot be registered the sink writes
    /// nothing and wants nothing, as the page says production code should carry on: [`is_registered`](Self::is_registered)
    /// says whether it did.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registration: shared().ok(),
        }
    }

    /// Whether the provider is registered, so that a session can listen for it.
    #[must_use]
    pub fn is_registered(&self) -> bool {
        self.registration.is_some()
    }

    /// Whether any sink of this process holds the provider's registration now. It is made by the first sink and given back
    /// when the last is dropped, so this is false before the first and after the last.
    #[must_use]
    pub fn provider_is_registered_in_process() -> bool {
        SHARED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .strong_count()
            > 0
    }
}

impl Default for EtwDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticSink for EtwDiagnostics {
    fn wants(&self, kind: DiagnosticKind) -> bool {
        let Some(registration) = &self.registration else {
            return false;
        };
        // The level and keyword are the same for each of a kind's events, so the kind is asked about by the first.
        schema_of_kind(kind)
            .is_some_and(|schema| registration.is_level_enabled(schema.level, schema.keyword.mask))
    }

    fn report(&self, event: &Diagnostic<'_>) {
        let Some(registration) = &self.registration else {
            return;
        };
        let Some(schema) = schema_of(event) else {
            return;
        };
        let pairing = event.pairing();
        // An end lets its pair's activity go whether or not it is written, so a session that stopped between a begin and its
        // end leaves nothing held.
        let ended = match pairing {
            Some(pairing) if pairing.edge == Edge::End => activity::ended(pairing.id),
            _ => None,
        };
        let descriptor = schema.descriptor();
        // Asked again, of the event itself, before anything is built: `wants` was asked of its kind.
        if !registration.is_enabled(&descriptor) {
            return;
        }
        let Some(values) = encode(event) else {
            return;
        };
        let fields: Vec<_> = values.iter().map(crate::encode::Value::field).collect();
        let activity = match pairing {
            None => None,
            Some(pairing) => match pairing.edge {
                Edge::Begin => activity::begun(pairing.id, || create_activity_id().ok()),
                Edge::End => ended.or_else(|| create_activity_id().ok()),
            },
        };
        // The page says the code "is primarily intended for use in debugging and diagnostic scenarios" and that most callers
        // ignore it, and a sink has no way to report a failure of its own.
        let _ = match activity {
            Some(activity) => {
                registration.write_transfer(&descriptor, Some(&activity), None, &fields)
            }
            None => registration.write(&descriptor, &fields),
        };
    }
}
