// Copyright (c) 2026 Mike Grier
//! Unit tests for per-subscription route matching.

use wtf_string::Wtf16String;

use std::cmp::Ordering;

use win_error_codes::Win32Error;
use win_text_sys::{CompareStringOrdinalError, Operand};

use super::{Route, RouteScope, equal_from, names_match};
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
    selected_with(route, names, false)
}

fn selected_with(route: &Route, names: &[&str], case_sensitive: bool) -> Vec<String> {
    let changes: Vec<Change> = names.iter().map(|n| change(n)).collect();
    route
        .select(&changes, case_sensitive)
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
fn a_file_route_comparison_is_exact_on_a_case_sensitive_directory() {
    // PR #20 review response: on a directory that opted into per-directory
    // case sensitivity (`fsutil file setCaseSensitiveInfo`), `A.txt` and
    // `a.txt` genuinely name different files -- folding case there would
    // route a different file's changes to this route.
    let route = route(RouteScope::File {
        leaf: Wtf16String::from_units(&"Target.txt".encode_utf16().collect::<Vec<u16>>()),
    });
    assert_eq!(
        selected_with(&route, &["target.txt", "TARGET.TXT", "Target.txt"], true),
        vec!["Target.txt"]
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
    assert!(route.select(&[], false).is_empty());
}

#[test]
fn order_is_preserved_within_a_selection() {
    let route = route(RouteScope::Directory { subtree: false });
    assert_eq!(
        selected(&route, &["a.txt", "sub\\hidden.txt", "b.txt", "c.txt"]),
        vec!["a.txt", "b.txt", "c.txt"]
    );
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn names_match_folds_case_only_when_the_directory_is_case_insensitive() {
    for (left, right) in [("a.txt", "A.TXT"), ("\u{e9}", "\u{c9}"), ("Mixed", "mIXED")] {
        assert!(names_match(&units(left), &units(right), false), "{left:?}");
        assert!(!names_match(&units(left), &units(right), true), "{left:?}");
    }
}

#[test]
fn names_match_agrees_with_itself_on_identical_names_in_both_modes() {
    for text in ["", "a", "target.txt", "\u{1f600}.txt", "a b"] {
        assert!(names_match(&units(text), &units(text), false), "{text:?}");
        assert!(names_match(&units(text), &units(text), true), "{text:?}");
    }
}

#[test]
fn names_match_rejects_different_names_in_both_modes() {
    for (left, right) in [("a", "b"), ("a", ""), ("", "a"), ("ab", "a"), ("a", "ab")] {
        assert!(!names_match(&units(left), &units(right), false), "{left:?}");
        assert!(!names_match(&units(left), &units(right), true), "{left:?}");
    }
}

#[test]
fn names_match_compares_an_interior_nul_as_content() {
    let with_b = [u16::from(b'a'), 0, u16::from(b'b')];
    let with_c = [u16::from(b'a'), 0, u16::from(b'c')];
    for case_sensitive in [false, true] {
        assert!(names_match(&with_b, &with_b, case_sensitive));
        assert!(!names_match(&with_b, &with_c, case_sensitive));
        assert!(!names_match(
            &[u16::from(b'a'), 0],
            &[u16::from(b'a')],
            case_sensitive
        ));
    }
}

#[test]
fn a_comparison_result_is_a_match_only_when_it_says_equal() {
    let a = [u16::from(b'a')];
    let b = [u16::from(b'b')];
    assert!(equal_from(Ok(Ordering::Equal), &a, &b));
    assert!(!equal_from(Ok(Ordering::Less), &a, &a));
    assert!(!equal_from(Ok(Ordering::Greater), &a, &a));
}

#[test]
fn a_name_too_long_for_the_call_is_compared_exactly() {
    let too_long = |len| {
        Err(CompareStringOrdinalError::TooLong {
            operand: Operand::First,
            len,
        })
    };
    let a = [u16::from(b'a')];
    let upper_a = [u16::from(b'A')];
    assert!(equal_from(too_long(1 << 32), &a, &a));
    assert!(!equal_from(too_long(1 << 32), &a, &upper_a));
}

#[test]
fn a_failure_of_the_call_is_not_a_match() {
    let failed = Err(CompareStringOrdinalError::Call(Win32Error::from_code(87)));
    let a = [u16::from(b'a')];
    assert!(!equal_from(failed, &a, &a));
}
