// Copyright (c) 2026 Mike Grier
//! Tests for [`remote_placement`], the one place `--placement remote` decides
//! where a domain's memory goes.
//!
//! Both directions, because the defect these exist for was a caller refusing
//! an outcome it should have honoured: [`RemoteNode::SameAsLocal`] places
//! locally, while the two outcomes that cannot say where "remote" is are
//! refused.

use super::{RemoteNode, remote_placement};
use win_numa_sys::NumaNode;

#[test]
fn another_node_is_placed_on() {
    let local = Some(NumaNode::new(0));
    assert_eq!(
        remote_placement(RemoteNode::Other(NumaNode::new(1)), local),
        Ok(Some(NumaNode::new(1)))
    );
}

#[test]
fn a_single_node_machine_places_locally_rather_than_refusing() {
    // The case `--compare` used to refuse straight after the preflight said it
    // would measure local placement.
    let local = Some(NumaNode::new(0));
    assert_eq!(remote_placement(RemoteNode::SameAsLocal, local), Ok(local));
}

#[test]
fn an_unnamed_topology_is_refused() {
    let local = Some(NumaNode::new(0));
    assert_eq!(
        remote_placement(RemoteNode::Unnamed, local),
        Err(RemoteNode::Unnamed)
    );
}

#[test]
fn an_unknown_local_node_is_refused() {
    assert_eq!(
        remote_placement(RemoteNode::LocalUnknown, None),
        Err(RemoteNode::LocalUnknown)
    );
}
