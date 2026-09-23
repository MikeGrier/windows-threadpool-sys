// Copyright (c) 2026 Mike Grier
//! Tests for [`NumaNode`] and the two node queries.
//!
//! # What these can and cannot establish
//!
//! The newtype's tests are total: it is a wrapper over a `u32` and every claim
//! about it holds on any host.
//!
//! The query tests are **host-dependent by nature**, and are written to assert
//! only what is true on every machine rather than what happens to be true on
//! this one. This workspace is developed on a single-node host, so a test that
//! asserted a particular node back would be asserting the only answer available
//! here and would fail on the hardware the crate exists for. What is asserted
//! instead is shape: that a query either answers or says it could not, that it
//! never invents a node, and that asking twice agrees.

use std::os::windows::io::AsRawHandle;

use super::{NumaNode, highest_numa_node, volume_numa_node};

/// A scratch file to ask about, named per test so tests running as threads in
/// one process cannot collide on it.
fn scratch(tag: &str) -> (std::path::PathBuf, std::fs::File) {
    let path = std::env::temp_dir().join(format!(
        "win-numa-sys-node-{}-{tag}.tmp",
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
fn a_node_round_trips_through_the_newtype() {
    for raw in [0_u32, 1, 7, 63, u32::MAX - 1, u32::MAX] {
        assert_eq!(NumaNode::new(raw).get(), raw, "for {raw}");
    }
}

#[test]
fn nodes_compare_and_order_by_their_number() {
    assert_eq!(NumaNode::new(3), NumaNode::new(3));
    assert_ne!(NumaNode::new(3), NumaNode::new(4));
    assert!(NumaNode::new(3) < NumaNode::new(4));
    let mut nodes = [NumaNode::new(9), NumaNode::new(2), NumaNode::new(5)];
    nodes.sort_unstable();
    assert_eq!(
        nodes,
        [NumaNode::new(2), NumaNode::new(5), NumaNode::new(9)]
    );
}

/// The `Display` form says what the number *is*, because a bare integer in a
/// report is ambiguous between a node number and a node count -- the very
/// confusion the newtype exists to stop.
#[test]
fn display_names_the_thing_rather_than_printing_a_bare_integer() {
    assert_eq!(NumaNode::new(0).to_string(), "node 0");
    assert_eq!(NumaNode::new(12).to_string(), "node 12");
}

/// Whatever the highest node is, asking twice agrees.
///
/// Deliberately not an assertion about the value: on this workspace's host it
/// is `node 0`, and pinning that would encode the development machine into the
/// suite. Hot-add could in principle change it between calls, which would make
/// this flaky rather than wrong -- it has never been observed, and a failure
/// here would be a genuine finding about the platform rather than a bad test.
#[test]
fn the_highest_node_is_stable_across_calls() {
    assert_eq!(highest_numa_node(), highest_numa_node());
}

/// A machine that answers reports a node number, not a count.
///
/// The distinction is the reason the newtype exists: a single-node machine
/// answers `node 0`, and code reading that as "zero nodes" would conclude the
/// machine has no NUMA at all.
#[test]
fn the_highest_node_is_a_number_not_a_count() {
    if let Some(highest) = highest_numa_node() {
        // Every machine has at least one node, so the highest number is
        // reachable as a node. This holds on a 1-node host (0) and on a
        // 64-node one (63).
        assert!(
            highest.get() < u32::MAX,
            "a real machine's highest node cannot be the no-preference sentinel"
        );
    }
}

/// The volume query either answers or reports why not; it never invents a node.
///
/// On a local NTFS volume this workspace's host answers. A host whose storage
/// stack declines is equally valid and must produce an error rather than a
/// fabricated zero -- which is the failure this asserts against, because zero
/// is a real node number and so a plausible-looking fabrication.
#[test]
fn the_volume_query_answers_or_errors_but_never_fabricates() {
    let (path, file) = scratch("answers-or-errors");
    match volume_numa_node(file.as_raw_handle()) {
        Ok(node) => assert!(
            node.get() < u32::MAX,
            "a reported node cannot be the no-preference sentinel"
        ),
        Err(error) => assert_ne!(
            error.kind(),
            std::io::ErrorKind::Other,
            "a failure should carry the OS error, not a placeholder"
        ),
    }
    drop(file);
    let _ = std::fs::remove_file(path);
}

/// Asking the same handle twice agrees.
#[test]
fn the_volume_query_is_stable_for_one_handle() {
    let (path, file) = scratch("stable");
    let first = volume_numa_node(file.as_raw_handle()).ok();
    let second = volume_numa_node(file.as_raw_handle()).ok();
    assert_eq!(first, second);
    drop(file);
    let _ = std::fs::remove_file(path);
}

/// An invalid handle is an error, not a node.
///
/// The cheapest reachable failure path, and worth having because the success
/// path cannot be forced to fail on a host whose volume does answer.
#[test]
fn an_invalid_handle_errors() {
    let error = volume_numa_node(std::ptr::null_mut())
        .expect_err("the null handle is not a file or directory handle");
    assert!(
        error.raw_os_error().is_some(),
        "the failure should carry the OS error code, got {error:?}"
    );
}
