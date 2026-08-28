// Copyright (c) 2026 Mike Grier
//! Small helpers shared by this module's own test files.

use crate::globazog_adapter::types::DirEntry;

/// Reconstruct an ASCII-only name from its decoded code points, for
/// identifying an entry by the (deliberately ASCII) names these tests
/// create. Never used by a test whose subject *is* non-ASCII round-tripping,
/// which compares code points directly instead.
pub fn ascii_name(entry: &DirEntry) -> String {
    entry
        .name
        .iter()
        .map(|&cp| char::from_u32(cp).unwrap_or('\u{FFFD}'))
        .collect()
}
