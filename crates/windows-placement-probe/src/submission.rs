// Copyright (c) 2026 Mike Grier
//! Turning a run into something a person can paste into a discussion thread.
//!
//! # The paste is the channel
//!
//! Results are collected by asking people to paste a run into a GitHub
//! Discussions thread. That makes the terminal output the submission, so it has
//! to survive being selected, copied, and dropped into a comment box.
//!
//! **The target is select-all, copy, paste, done.** Every additional step is a
//! submission that does not arrive, which is why this module emits its own
//! markdown fences: a runner who has never thought about markdown pastes the
//! whole thing and it renders as a code block regardless. A few lines of
//! instruction caught inside the fence are trivial noise beside a paste that
//! renders as mangled prose.
//!
//! # Why a checksum
//!
//! A paste can be truncated by a scrollback limit, reflowed by a narrow
//! terminal, or half-selected by a mouse. The checksum makes that **detectable**
//! rather than silently half-ingested -- the same principle as the schema
//! golden, which detects a shape change rather than trusting nobody caused one.
//!
//! It is a non-cryptographic digest and defends against accident, not against a
//! person who wants to submit a false record. That threat is not in scope: the
//! cost of a fabricated placement measurement is a wrong row in a table that
//! disagrees with every other host, which is visible.

use crate::record::SubmissionRecord;
use crate::report;

/// Where a runner is asked to send a result.
pub const DISCUSSION_URL: &str = "https://github.com/MikeGrier/windows-threadpool-sys/discussions";

/// A 64-bit FNV-1a digest, rendered as sixteen hex characters.
///
/// FNV-1a because it needs no dependency, is a dozen lines, and is entirely
/// adequate for catching a truncated or reflowed paste. It is **not** a
/// security control and the output says so rather than letting a reader assume
/// otherwise.
#[must_use]
pub fn checksum(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

/// The complete text a runner is asked to paste.
///
/// # Errors
///
/// Returns whatever serializing the record failed with.
pub fn render_submission(record: &SubmissionRecord) -> Result<String, serde_json::Error> {
    // Pretty-printed rather than compact. A collector parses either, but a
    // *person* is being asked to look at this before sending it, and a single
    // enormous line is both unreadable and the most likely thing a terminal
    // will wrap.
    let json = serde_json::to_string_pretty(record)?;
    let digest = checksum(json.as_bytes());

    let mut out = String::new();
    out.push_str("Paste EVERYTHING below into a reply at:\n");
    out.push_str(DISCUSSION_URL);
    out.push_str("\n\n");

    // The fence is emitted by the tool so the runner does not have to know it
    // is needed.
    out.push_str("```text\n");
    out.push_str(&report::render(record));
    out.push_str("\n-- the record --\n");
    out.push_str("(checksum ");
    out.push_str(&digest);
    out.push_str(", FNV-1a over the JSON below; it catches a truncated or\n");
    out.push_str("reflowed paste, and is not a security control)\n\n");
    out.push_str(&json);
    out.push_str("\n```\n");

    Ok(out)
}

/// A predictable file name for the same record.
///
/// Includes the timestamp so a second run does not overwrite a first, and the
/// schema version so a directory of collected files can be sorted by shape
/// without opening any of them.
#[must_use]
pub fn file_name(record: &SubmissionRecord) -> String {
    let stamp: String = record
        .recorded_at
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("placement-probe-v{}-{}.json", record.schema_version, stamp)
}

#[cfg(test)]
mod tests;
