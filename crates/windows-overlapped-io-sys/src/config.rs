//! Process-wide diagnostic configuration.
//!
//! Per-operation source tracking records where each operation was submitted so
//! the drop-time rundown diagnostic can name the sources of any operation still
//! outstanding. It is off by default because, when on, it takes a mutex on the
//! submission hot path. Rundown correctness never depends on it.

use std::sync::OnceLock;

/// The name of the environment variable that provides the default setting.
const ENV_VAR: &str = "WINDOWS_OVERLAPPED_IO_SYS_TRACK";

static SOURCE_TRACKING: OnceLock<bool> = OnceLock::new();

/// Returned when source tracking is configured after it has already been set.
#[derive(Debug)]
pub struct SourceTrackingAlreadySet;

impl std::fmt::Display for SourceTrackingAlreadySet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operation source tracking was already configured")
    }
}

impl std::error::Error for SourceTrackingAlreadySet {}

/// Enable or disable per-operation source tracking for the whole process.
///
/// This is a one-time in-process setting: it must be called before the first
/// operation is submitted and before any call to [`source_tracking_enabled`], or
/// it returns [`SourceTrackingAlreadySet`]. When it is never called, the setting
/// defaults from the `WINDOWS_OVERLAPPED_IO_SYS_TRACK` environment variable,
/// which enables tracking when set to `1`, `true`, `on`, or `yes`.
///
/// # Errors
///
/// Returns [`SourceTrackingAlreadySet`] if the setting has already been resolved.
pub fn set_source_tracking(enabled: bool) -> Result<(), SourceTrackingAlreadySet> {
    SOURCE_TRACKING
        .set(enabled)
        .map_err(|_| SourceTrackingAlreadySet)
}

/// Whether per-operation source tracking is enabled for this process.
///
/// The first call resolves the setting from the environment if it was not set
/// explicitly, and the result is then fixed for the rest of the process.
#[must_use]
pub fn source_tracking_enabled() -> bool {
    *SOURCE_TRACKING.get_or_init(default_from_env)
}

fn default_from_env() -> bool {
    match std::env::var(ENV_VAR) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        Err(_) => false,
    }
}
