// Copyright (c) Mike Grier.

//! Tests for the shape-construction options.
//!
//! The builder itself is exercised wherever a shape is built with options; what
//! is tested here is the rendering, which a mutation run found unguarded: a
//! `Debug` returning `Ok(default)` writes nothing and passes any test that only
//! checks formatting does not panic.

use super::Options;
use crate::Disposal;

#[test]
fn the_debug_rendering_shows_both_options_and_tracks_them_changing() {
    // Both fields and both states of each, so a rendering stuck at one constant
    // cannot satisfy this.
    let bare = format!("{:?}", Options::<u32>::new());
    assert!(bare.contains("Options"), "got {bare}");
    assert!(
        bare.contains("false"),
        "a fresh Options has no disposal and no tracking: {bare}"
    );

    let configured = format!(
        "{:?}",
        Options::<u32>::new()
            .tracking_high_water()
            .disposal(Disposal::new(|_: u32| {}))
    );
    assert!(
        configured.contains("true"),
        "a configured Options must show it: {configured}"
    );
    assert!(
        !configured.contains("false"),
        "both fields were set, so neither should still read false: {configured}"
    );
}
