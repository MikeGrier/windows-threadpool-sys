// Copyright (c) Mike Grier
//! One doc comment per item, checked by reading the source.
//!
//! **The defect this exists for.** An edit that replaces an item's
//! documentation by anchoring on its signature and prepending new prose leaves
//! the previous block above the new one. The result compiles, renders, and
//! passes every gate this crate has: rustdoc validates link syntax rather than
//! prose, clippy does not read doc semantics, and `missing_docs` is satisfied
//! by the presence of *a* comment rather than by its being the right one.
//!
//! Eight of them accumulated on one branch before a reviewer found them by
//! reading. Five said a field was reached by a trampoline that had stopped
//! touching it; two left the item whose doc had been stolen with none at all;
//! one put "this brings a process-wide hazard forward" above a method that
//! avoids that hazard.
//!
//! **Only the mechanically sound half is checked here, and that is deliberate.**
//! A doc block carrying the same heading twice is always wrong -- there is no
//! legitimate item with two `# Safety` sections -- so that is an error. The
//! general case, two summary sentences fused, has no sound signal: a heuristic
//! over "a one-sentence paragraph with no blank line before it" flags about
//! twenty-five sites in this crate, of which four are real. Shipping that as a
//! test would be shipping an alarm nobody can act on. It is a useful one-off
//! grep and is written down in `M-T12.4` rather than automated.

#![cfg(windows)]

use std::path::{Path, PathBuf};

/// Every `.rs` file under `src`, found recursively.
///
/// Recursive rather than a single `read_dir`, because this crate's modules
/// nest: a shallow walk would silently skip `heal/`, `timer/` and `trace/`,
/// and report success over a fraction of the tree. That exact mistake is
/// recorded in this repository's instructions as one that has already shipped.
fn sources(root: &Path, into: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(root).unwrap_or_else(|why| panic!("read {root:?}: {why}"));
    for entry in entries {
        let path = entry.expect("read a directory entry").path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            into.push(path);
        }
    }
}

/// A heading inside a doc block, or `None` if the line is not one.
///
/// `content` is the text after the `///` or `//!` marker. Callers must already
/// have excluded lines inside a fenced code block: a doctest hides a line by
/// prefixing it with `# `, which is indistinguishable from a heading here and
/// would otherwise make every example with two hidden lines a false report.
fn heading(content: &str) -> Option<&str> {
    let rest = content.trim_start_matches('#');
    let hashes = content.len() - rest.len();
    if (1..=6).contains(&hashes) && rest.starts_with(' ') && !rest.trim().is_empty() {
        Some(content.trim())
    } else {
        None
    }
}

// encoding-check: allow-glued-doc-comment
//
// This file parses doc comments, so it necessarily contains a quote welded to a
// doc marker -- in `strip_prefix` below, and in every test case that feeds this
// function a sample line. That is the exact sequence `tools/check-encoding.ps1`
// flags as a mis-joined edit, and nothing textual separates the two cases. The
// marker turns off that one rule for this file; encoding, control characters
// and mojibake are still checked here.

/// The text after a doc marker, or `None` if the line is not a doc comment.
fn doc_content(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("///")
        .or_else(|| trimmed.strip_prefix("//!"))?;
    // A bare `///` has no following space; `/// x` has one. Either is a doc
    // line, and `////` separators are not.
    if rest.starts_with('/') {
        return None;
    }
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

#[test]
fn no_doc_block_carries_the_same_heading_twice() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);

    // A walk that found nothing would pass this test over an empty set. The
    // number is a floor rather than a count, so adding or removing a module
    // does not touch it.
    assert!(
        files.len() >= 10,
        "the walk found only {} source files under {root:?}, which is too few for this \
         crate -- the walk is broken and every assertion below would be vacuous",
        files.len()
    );

    let mut headings_seen = 0_usize;
    let mut faults = Vec::new();

    for file in &files {
        let text = std::fs::read_to_string(file).expect("read a source file");
        let name = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();

        let mut block_start = 0_usize;
        let mut in_block = false;
        let mut fenced = false;
        let mut found: Vec<String> = Vec::new();

        for (index, line) in text.lines().enumerate() {
            let Some(content) = doc_content(line) else {
                // The block ended. Its headings are judged as a unit, because
                // the same heading in two *different* blocks is ordinary.
                in_block = false;
                fenced = false;
                found.clear();
                continue;
            };
            if !in_block {
                in_block = true;
                block_start = index + 1;
                fenced = false;
                found.clear();
            }
            if content.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }
            if let Some(found_heading) = heading(content) {
                headings_seen += 1;
                if found.iter().any(|seen| seen == found_heading) {
                    faults.push(format!(
                        "{name}:{block_start}: the doc block carries `{found_heading}` twice \
                         (second at line {}), which means two comments have been fused into \
                         one -- the item below it is documented partly by a comment written \
                         for something else",
                        index + 1
                    ));
                } else {
                    found.push(found_heading.to_owned());
                }
            }
        }
    }

    // The rejecting direction is asserted below; this is the accepting one. A
    // parser that recognised no headings at all would report no faults and look
    // exactly like a clean tree.
    assert!(
        headings_seen >= 20,
        "only {headings_seen} headings were recognised across {} files, which is far fewer \
         than this crate has -- the parser is not seeing headings, so finding no duplicates \
         says nothing",
        files.len()
    );

    assert!(
        faults.is_empty(),
        "{} fused doc block(s):\n{}",
        faults.len(),
        faults.join("\n")
    );
}

/// The check rejects a duplicate, which is the half the crate's own state
/// cannot demonstrate.
///
/// `no_doc_block_carries_the_same_heading_twice` passes because the tree is
/// clean, and a check that could never fail would pass for the same reason. So
/// the two helpers it is built from are exercised directly against the shape
/// they exist to catch.
#[test]
fn the_duplicate_heading_check_recognises_what_it_looks_for() {
    assert_eq!(heading("# Safety"), Some("# Safety"));
    assert_eq!(heading("## Panics"), Some("## Panics"));
    assert_eq!(heading("###### Deep"), Some("###### Deep"));

    assert_eq!(heading("Safety"), None, "prose is not a heading");
    assert_eq!(heading("#NoSpace"), None, "a heading needs its space");
    assert_eq!(heading("#"), None, "a bare hash is not a heading");
    assert_eq!(
        heading("####### Seven"),
        None,
        "markdown has six heading levels; seven hashes is not one of them"
    );
    assert_eq!(
        heading("# Ok::<(), std::io::Error>(())"),
        Some("# Ok::<(), std::io::Error>(())"),
        "a doctest's hidden line is shaped exactly like a heading, which is why the caller \
         must skip fenced regions rather than relying on this to tell them apart"
    );

    assert_eq!(doc_content("/// text"), Some("text"));
    assert_eq!(doc_content("    //! text"), Some("text"));
    assert_eq!(doc_content("///"), Some(""));
    assert_eq!(
        doc_content("// text"),
        None,
        "an ordinary comment is exempt"
    );
    assert_eq!(
        doc_content("//// text"),
        None,
        "a rule of slashes is not a doc"
    );
    assert_eq!(doc_content("let x = 1;"), None);
}
