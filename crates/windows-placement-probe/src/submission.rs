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

use crate::paste_json;
use crate::record::SubmissionRecord;
use crate::report;

/// Where a runner is asked to send a result.
///
/// **The specific thread, not the discussions index.** Someone doing a favour
/// should land on the reply box rather than on a list they have to search: a
/// link costing an extra navigation is one more place a submission stops.
pub const DISCUSSION_URL: &str =
    "https://github.com/MikeGrier/windows-threadpool-sys/discussions/55";

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
    // Laid out for a person rather than for a parser. A collector parses
    // either, but a *person* is being asked to look at this before sending it,
    // and both a single enormous line and eight lines saying `2` fail that.
    // See `paste_json` for the rule; every caller asks it rather than choosing
    // a layout of their own, so the checksum and the file cannot disagree with
    // what was printed.
    let json = paste_json::to_paste_json(record)?;
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
/// Includes the schema version so a directory of collected files can be sorted
/// by shape without opening any of them, and the timestamp -- when the record
/// carries one -- so two runs are distinguishable at a glance.
///
/// # Why the name carries milliseconds when the record does not
///
/// The record's own timestamp is floored to the minute, deliberately: a finer
/// one correlates two submissions from one host. A *file name* has a different
/// job and never leaves the runner's machine unless they send it, so it can be
/// as precise as the collision problem needs -- a run finishes in well under a
/// second, and two of them would otherwise want the same name.
///
/// The milliseconds come from the same clock reading as `recorded_at` rather
/// than from a fresh one, so the name and the record inside it always describe
/// the same instant -- and the record's published shape is unchanged, because
/// precision a file name wants is not a promise a record makes.
///
/// **This makes a collision unlikely, not impossible**, which is why it is not
/// the guarantee. Two runs can still start in the same millisecond, so the
/// writer creates the file exclusively and falls back to a numbered suffix;
/// that is what makes the guarantee, and this only keeps the suffix from ever
/// being needed in practice.
///
/// # A withheld timestamp is withheld from the name too
///
/// The name is derived from the record, so a record carrying no timestamp
/// yields a name carrying none -- `placement-probe-v1-250.json`. Reaching past
/// the record for the clock would put the withheld minute back into a file name
/// the runner may well attach, which is the one place it could still escape.
///
/// Only convenience is lost: the collision *guarantee* was never the stamp, and
/// the milliseconds that keep the suffix unnecessary are still here.
#[must_use]
pub fn file_name(record: &SubmissionRecord) -> String {
    // The trailing separator belongs to the stamp rather than to the format
    // string, so an absent stamp leaves no doubled hyphen behind.
    let stamp: String = match &record.recorded_at {
        Some(at) => at
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .chain(std::iter::once('-'))
            .collect(),
        None => String::new(),
    };
    format!(
        "placement-probe-v{}-{}{:03}.json",
        record.schema_version, stamp, record.recorded_at_subsecond_millis
    )
}

#[cfg(test)]
mod tests;
