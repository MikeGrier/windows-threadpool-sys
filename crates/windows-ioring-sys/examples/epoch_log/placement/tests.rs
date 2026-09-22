// Copyright (c) 2026 Mike Grier
//! Tests for the arena's placement decision (M22.3).
//!
//! # What these can and cannot establish
//!
//! They establish that the decision is *made and reported honestly*: that the
//! FSCTL is actually asked, that a node it reports is the node the arena would
//! be given, and that the report line says what the answer is worth on the
//! machine running it.
//!
//! They establish **nothing about locality**, and cannot. A single-node host
//! has one answer, so no test here can distinguish a good placement from the
//! only placement available; and the allocator's parameter is `nndPreferred`,
//! so even a multi-node host would not prove from a success that the pages
//! landed where they were asked for. Settling that needs hardware this is not
//! developed on, which is a hardware gap rather than a deferred decision.

use std::os::windows::io::AsRawHandle;

use super::Placement;

/// A scratch file to ask about, named per test so tests running as threads in
/// one process cannot collide on it.
fn scratch(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-epoch-placement-{}-{tag}.tmp",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .expect("a scratch file in the temp directory");
    (path, file)
}

#[test]
fn an_ordinary_file_gets_a_decision_either_way() {
    let (path, file) = scratch("ordinary");
    let placement = Placement::decide(file.as_raw_handle());

    // Both arms are legitimate outcomes -- a volume that names no node is the
    // documented case, not a failure -- so what is asserted is that the
    // decision and its description agree, never which arm was taken.
    match &placement {
        Placement::OnVolumeNode { .. } => {
            assert!(placement.node().is_some(), "a named node is offered");
        }
        Placement::Unplaced { .. } => {
            assert!(placement.node().is_none(), "no node is offered");
        }
    }

    drop(file);
    let _ = std::fs::remove_file(path);
}

#[test]
fn the_description_always_says_which_way_it_went() {
    let (path, file) = scratch("describes");
    let placement = Placement::decide(file.as_raw_handle());
    let described = placement.describe();

    assert!(
        described.contains("arena"),
        "the line names what was placed: {described}"
    );
    match placement.node() {
        Some(node) => assert!(
            described.contains(&node.to_string()),
            "a placed arena names its node: {described}"
        ),
        None => assert!(
            described.contains("no NUMA preference"),
            "an unplaced arena says so rather than staying quiet: {described}"
        ),
    }

    drop(file);
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_single_node_machine_is_told_the_placement_bought_nothing() {
    // The honesty guard. On a one-node machine "placed on node 0" is true and
    // misleading, so the description must say the choice was not available.
    // Constructed rather than queried, so the assertion holds on any host.
    let described = Placement::OnVolumeNode {
        node: 0,
        highest_node: Some(0),
    }
    .describe();
    assert!(
        described.contains("changes nothing here"),
        "a one-node machine must be told so: {described}"
    );
}

#[test]
fn a_multi_node_machine_is_not_told_that() {
    // The other direction: the disclaimer above must not appear where it would
    // be false, or it would train a reader to ignore it.
    let described = Placement::OnVolumeNode {
        node: 1,
        highest_node: Some(3),
    }
    .describe();
    assert!(
        !described.contains("changes nothing here"),
        "a multi-node machine must not be told the placement was moot: {described}"
    );
    assert!(
        described.contains("0..=3"),
        "and it says what the choice was made from: {described}"
    );
}

#[test]
fn an_unknown_node_count_is_admitted_rather_than_assumed() {
    let described = Placement::OnVolumeNode {
        node: 2,
        highest_node: None,
    }
    .describe();
    assert!(
        described.contains("could not be determined"),
        "an unknown node count is said, not guessed: {described}"
    );
    assert!(
        !described.contains("changes nothing here"),
        "and it is not silently treated as a single-node machine: {described}"
    );
}

#[test]
fn an_unplaced_arena_reports_why() {
    // A reader must be able to tell "asked and got no answer" from "never
    // asked", which is the distinction the reason carries.
    let described = Placement::Unplaced {
        reason: std::io::Error::from_raw_os_error(1),
    }
    .describe();
    assert!(described.contains("does not report a node"), "{described}");
    assert!(
        described.len() > "arena allocated with no NUMA preference: ".len() + 40,
        "the underlying error is included, not swallowed: {described}"
    );
}

#[test]
fn an_unplaced_arena_offers_no_node() {
    let placement = Placement::Unplaced {
        reason: std::io::Error::from_raw_os_error(1),
    };
    assert_eq!(placement.node(), None);
}

#[test]
fn a_placed_arena_offers_the_node_it_named() {
    for node in [0_u32, 1, 7, 63] {
        let placement = Placement::OnVolumeNode {
            node,
            highest_node: Some(63),
        };
        assert_eq!(
            placement.node(),
            Some(node),
            "the node that is reported is the node that is used"
        );
    }
}

#[test]
fn an_invalid_handle_is_an_unplaced_arena_not_a_panic() {
    // The sample must survive a handle the FSCTL refuses: placement is an
    // optimisation, and failing to make it is never a reason to fail the log.
    let placement = Placement::decide(std::ptr::null_mut());
    assert!(
        matches!(placement, Placement::Unplaced { .. }),
        "a refused query is a decision, not an error path"
    );
    assert_eq!(placement.node(), None);
}
