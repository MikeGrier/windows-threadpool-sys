// Copyright (c) Mike Grier.

//! The worker-context probe's machine-readable row (`M8.9`).

use super::*;
use crate::row::{Row, Value};

#[cfg(test)]
mod tests;

/// What the row names itself.
pub const REASON: &str = "x-probe-worker-context";

/// The row a report carries.
///
/// **Raw data**, per `d-every-probe-emits-a-row` in DESIGN-NOTES.md: what a
/// worker found when submitted plainly, and what the submitter and its worker
/// each found when submitted while impersonating -- for each, whether
/// `OpenThreadToken` found a token, its `GetLastError` when it did not (`null`
/// when it did, since none was read), and the thread error mode on entry.
///
/// Not unimpersonated, disagree or handler-enabled; each is a comparison over
/// the figures here. The probe has no way to decline: a callback that never
/// reports stops it with a panic after its timeout, and a panic prints no
/// report.
#[must_use]
pub fn raw_row(plain: WorkerContext, impersonating: IdentityAsymmetry, build: &str) -> Row {
    Row::new(REASON)
        .with("arch", std::env::consts::ARCH)
        .with("build", build)
        .with("plain_worker", context(plain))
        .with(
            "impersonating",
            Value::Object(vec![
                ("submitter", context(impersonating.submitter)),
                ("worker", context(impersonating.worker)),
            ]),
        )
}

fn context(seen: WorkerContext) -> Value {
    Value::Object(vec![
        ("has_thread_token", Value::from(seen.has_thread_token)),
        (
            "open_token_error",
            Value::from((!seen.has_thread_token).then_some(seen.open_token_error)),
        ),
        ("error_mode", Value::from(seen.error_mode)),
    ])
}
