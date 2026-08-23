// Copyright (c) 2026 Mike Grier
//! Unit tests for per-subscription route matching.

use wtf_string::Wtf16String;

use super::{Route, RouteScope};
use crate::notify::{Change, ChangeKind, RelativeName};
use crate::queue::{WatchId, channel};
use crate::watch::{RetryMode, VolumeChangePolicy};

fn name(units: &str) -> RelativeName {
    RelativeName::from_units(units.encode_utf16().collect())
}

fn change(units: &str) -> Change {
    Change {
        kind: ChangeKind::Added,
        name: name(units),
    }
}

fn route(scope: RouteScope) -> Route {
    let (sink, _receiver) = channel();
    Route {
        watch: WatchId::from_raw(1),
        scope,
        sink,
        retry: RetryMode::Defaults,
        report_liveness: false,
        on_volume_change: VolumeChangePolicy::AutoContinue,
        fault_slot: None,
    }
}

fn selected(route: &Route, names: &[&str]) -> Vec<String> {
    let changes: Vec<Change> = names.iter().map(|n| change(n)).collect();
    route
        .select(&changes)
        .into_iter()
        .map(|c| c.name.to_os_string().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_recursive_directory_route_matches_every_depth() {
    let route = route(RouteScope::Directory { subtree: true });
    assert_eq!(
        selected(&route, &["a.txt", "sub\\b.txt", "sub\\deep\\c.txt"]),
        vec!["a.txt", "sub\\b.txt", "sub\\deep\\c.txt"]
    );
}

#[test]
fn a_shallow_directory_route_matches_only_direct_children() {
    let route = route(RouteScope::Directory { subtree: false });
    assert_eq!(selected(&route, &["a.txt", "sub\\b.txt"]), vec!["a.txt"]);
}

#[test]
fn a_file_route_matches_only_its_own_leaf() {
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"target.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(
        selected(&route, &["target.txt", "other.txt"]),
        vec!["target.txt"]
    );
}

#[test]
fn a_file_route_never_matches_below_the_directory() {
    // A file target is always a direct child of the directory that was opened
    // (D-7); a nested file of the same leaf name is a different file.
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"target.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(selected(&route, &["sub\\target.txt"]), Vec::<String>::new());
}

#[test]
fn a_file_route_match_is_exact_not_a_prefix_or_substring() {
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"target.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(
        selected(&route, &["target.txt.bak", "not-target.txt", "target.tx"]),
        Vec::<String>::new()
    );
}

#[test]
fn a_file_route_comparison_is_case_insensitive_over_ascii() {
    // The default Windows filesystem is case-insensitive but
    // case-preserving: `CreateFileW` accepts a target regardless of casing,
    // so a decoded notification's stored casing must still match a route
    // whose subscribe path used different casing.
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"Target.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(
        selected(&route, &["target.txt", "TARGET.TXT", "TaRgEt.TxT"]),
        vec!["target.txt", "TARGET.TXT", "TaRgEt.TxT"]
    );
}

#[test]
fn a_file_route_comparison_is_case_insensitive_over_non_ascii_letters() {
    // PR #20 review response: an ASCII-only fold silently dropped a match
    // whenever the differing case fell outside ASCII, e.g. a stored `E.txt`
    // opened through a subscription spelling `e.txt` -- exactly the kind of
    // event this crate's completeness contract (D-77) promises never to
    // drop. `CompareStringOrdinal`'s ordinal case folding matches both.
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"\u{c9}.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(
        selected(&route, &["\u{e9}.txt", "\u{c9}.txt"]),
        vec!["\u{e9}.txt", "\u{c9}.txt"]
    );
}

#[test]
fn a_file_route_comparison_still_requires_the_same_length() {
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"target.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(
        selected(&route, &["target.tx", "target.txtx"]),
        Vec::<String>::new()
    );
}

#[test]
fn only_a_recursive_directory_route_needs_kernel_subtree() {
    assert!(RouteScope::Directory { subtree: true }.needs_kernel_subtree());
    assert!(!RouteScope::Directory { subtree: false }.needs_kernel_subtree());
    assert!(
        !RouteScope::File {
            leaf: Wtf16String::from_units(&[])
        }
        .needs_kernel_subtree()
    );
}

#[test]
fn selecting_from_an_empty_batch_yields_nothing() {
    let route = route(RouteScope::Directory { subtree: true });
    assert!(route.select(&[]).is_empty());
}

#[test]
fn order_is_preserved_within_a_selection() {
    let route = route(RouteScope::Directory { subtree: false });
    assert_eq!(
        selected(&route, &["a.txt", "sub\\hidden.txt", "b.txt", "c.txt"]),
        vec!["a.txt", "b.txt", "c.txt"]
    );
}
