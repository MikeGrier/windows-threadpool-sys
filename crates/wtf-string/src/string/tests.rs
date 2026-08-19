// Copyright (c) 2026 Mike Grier
use super::{Wtf16Str, Wtf16String};

// The tests reach into the private `units` field (a child module may) to assert
// the always-present terminator that the public API deliberately hides.

#[test]
fn new_holds_only_the_terminator() {
    let s = Wtf16String::new();
    assert_eq!(s.units, vec![0u16]);
    assert_eq!(s.len(), 0);
    assert!(s.is_empty());
    assert!(s.as_units().is_empty());
}

#[test]
fn default_matches_new() {
    assert_eq!(Wtf16String::default().units, Wtf16String::new().units);
}

#[test]
fn from_units_appends_the_terminator() {
    let s = Wtf16String::from_units(&[97, 98]);
    assert_eq!(s.units, vec![97, 98, 0]);
    assert_eq!(s.len(), 2);
    assert_eq!(s.as_units(), &[97, 98]);
}

#[test]
fn span_never_includes_the_terminator() {
    let s = Wtf16String::from_units(&[1, 2, 3]);
    assert_eq!(s.as_units().len(), s.units.len() - 1);
    assert_eq!(*s.units.last().unwrap(), 0);
    assert_eq!(s.as_units(), &[1, 2, 3]);
}

#[test]
fn empty_from_units_is_just_the_terminator() {
    let s = Wtf16String::from_units(&[]);
    assert_eq!(s.units, vec![0u16]);
    assert!(s.is_empty());
    assert!(s.as_units().is_empty());
}

#[test]
fn clone_preserves_the_invariant() {
    let s = Wtf16String::from_units(&[65, 66, 67]);
    let c = s.clone();
    assert_eq!(c.units, s.units);
    assert_eq!(*c.units.last().unwrap(), 0);
    assert_eq!(c.as_units(), s.as_units());
}

#[test]
fn interior_nul_content_is_preserved_and_reported() {
    let s = Wtf16String::from_units(&[97, 0, 98]);
    assert_eq!(s.units, vec![97, 0, 98, 0]);
    assert_eq!(s.as_units(), &[97, 0, 98]);
    assert!(s.has_interior_nul());
}

#[test]
fn no_interior_nul_is_reported_false() {
    assert!(!Wtf16String::from_units(&[97, 98]).has_interior_nul());
}

#[test]
fn borrowed_wraps_a_slice_without_copying() {
    let units = [97u16, 98, 99];
    let s = Wtf16Str::from_units(&units);
    assert_eq!(s.as_units(), &units);
    assert_eq!(s.len(), 3);
    assert!(!s.is_empty());
    assert_eq!(s.as_units().as_ptr(), units.as_ptr());
}

#[test]
fn borrowed_reports_interior_nul() {
    assert!(Wtf16Str::from_units(&[97, 0, 98]).has_interior_nul());
    assert!(!Wtf16Str::from_units(&[97, 98]).has_interior_nul());
}

#[test]
fn deref_and_to_owned_round_trip() {
    let content = [10u16, 20, 30, 40];
    let owned = Wtf16String::from_units(&content);
    let borrowed: &Wtf16Str = &owned;
    assert_eq!(borrowed.as_units(), &content);
    let owned2 = borrowed.to_owned();
    assert_eq!(owned2.units, owned.units);
}

#[test]
fn ill_formed_surrogates_survive_in_storage() {
    // A lone high surrogate is invalid UTF-16 but legal WTF-16; it must round-trip.
    let lone = [0xD800u16];
    let s = Wtf16String::from_units(&lone);
    assert_eq!(s.as_units(), &lone);
    assert_eq!(s.units, vec![0xD800, 0]);

    let pair = [0xD83Du16, 0xDE00]; // a well-formed astral pair, for contrast
    assert_eq!(Wtf16String::from_units(&pair).as_units(), &pair);
}

#[test]
fn large_input_keeps_one_terminator() {
    let content: Vec<u16> = (0..10_000).map(|i| (i % 65_535 + 1) as u16).collect();
    let s = Wtf16String::from_units(&content);
    assert_eq!(s.units.len(), content.len() + 1);
    assert_eq!(s.len(), content.len());
    assert_eq!(*s.units.last().unwrap(), 0);
    assert_eq!(s.as_units(), content.as_slice());
}
