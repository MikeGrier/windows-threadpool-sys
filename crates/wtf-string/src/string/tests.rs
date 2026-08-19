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

// --- M3: conversions, formatting, comparison ---

/// A representative spread of well-formed strings for round-trip checks.
fn well_formed_samples() -> Vec<&'static str> {
    vec![
        "",
        "a",
        "ascii only",
        "café",      // Latin-1 supplement (BMP, 2-byte UTF-8)
        "日本語",    // CJK (BMP, 3-byte UTF-8)
        "😀",        // astral (U+1F600, a UTF-16 surrogate pair)
        "aé日😀mix", // mixed BMP + astral
    ]
}

#[test]
fn from_str_encodes_to_utf16_units() {
    let s = Wtf16String::from("abc");
    assert_eq!(s.as_units(), &[97, 98, 99]);
    assert_eq!(*s.units.last().unwrap(), 0);
}

#[test]
fn from_string_matches_from_str() {
    assert_eq!(
        Wtf16String::from(String::from("hi ☃")).units,
        Wtf16String::from("hi ☃").units
    );
}

#[test]
fn round_trips_str_string_and_lossy() {
    for s in well_formed_samples() {
        let owned = Wtf16String::from(s);
        // Storage matches a direct UTF-16 encoding of the input.
        let expected: Vec<u16> = s.encode_utf16().collect();
        assert_eq!(owned.as_units(), expected.as_slice(), "units for {s:?}");
        // All three decode paths recover the original for well-formed input.
        assert_eq!(
            owned.to_string_checked().as_deref(),
            Some(s),
            "checked {s:?}"
        );
        assert_eq!(owned.to_string_lossy(), s, "lossy {s:?}");
        assert_eq!(owned.clone().into_string().unwrap(), s, "into_string {s:?}");
    }
}

#[test]
fn astral_pair_is_two_units_and_round_trips() {
    let owned = Wtf16String::from("😀");
    assert_eq!(owned.as_units().len(), 2); // one surrogate pair
    assert_eq!(owned.into_string().unwrap(), "😀");
}

#[test]
fn ill_formed_surrogate_checked_none_lossy_replaces() {
    let s = Wtf16Str::from_units(&[0xD800]);
    assert_eq!(s.to_string_checked(), None);
    assert_eq!(s.to_string_lossy(), "\u{FFFD}");
}

#[test]
fn into_string_returns_the_original_on_ill_formed() {
    let s = Wtf16String::from_units(&[0xD800]);
    match s.into_string() {
        Ok(decoded) => panic!("ill-formed content must not decode, got {decoded:?}"),
        Err(original) => assert_eq!(original.as_units(), &[0xD800]),
    }
}

#[test]
fn long_name_beyond_max_path_round_trips() {
    let long = "a".repeat(500);
    let owned = Wtf16String::from(long.as_str());
    assert_eq!(owned.as_units().len(), 500);
    assert_eq!(owned.into_string().unwrap(), long);
}

#[test]
fn display_is_lossy() {
    let s = Wtf16Str::from_units(&[0xD800, 97]); // ill-formed then 'a'
    assert_eq!(format!("{s}"), "\u{FFFD}a");
    assert_eq!(format!("{}", Wtf16String::from("plain")), "plain");
}

#[test]
fn debug_quotes_and_escapes() {
    assert_eq!(format!("{:?}", Wtf16String::from("ab")), "\"ab\"");
    assert_eq!(format!("{:?}", Wtf16String::from("a\nb")), "\"a\\nb\"");
}

#[test]
fn debug_escapes_lone_surrogate_losslessly() {
    // A lone surrogate is escaped OsStr-style, not collapsed to U+FFFD, so it
    // stays distinguishable from the real replacement character.
    assert_eq!(
        format!("{:?}", Wtf16Str::from_units(&[0xD800])),
        "\"\\u{d800}\""
    );
    assert_ne!(
        format!("{:?}", Wtf16Str::from_units(&[0xD800])),
        format!("{:?}", Wtf16Str::from_units(&[0xFFFD]))
    );
}

#[test]
fn ordering_is_binary_over_units() {
    let a = Wtf16String::from("a");
    let b = Wtf16String::from("b");
    let ab = Wtf16String::from("ab");
    let same1 = Wtf16String::from("same");
    let same2 = Wtf16String::from("same");
    assert!(a < b);
    assert!(a < ab); // a prefix orders before the longer string
    assert_eq!(same1, same2);
}

#[test]
fn equality_and_hash_are_consistent_for_borrow() {
    use std::collections::HashSet;
    let mut set: HashSet<Wtf16String> = HashSet::new();
    set.insert(Wtf16String::from("key"));
    let probe = Wtf16String::from("key");
    // Borrow<WtfStr> + matching Hash/Eq lets a borrowed slice find the owned key.
    assert!(set.contains(&*probe));
    assert!(!set.contains(&*Wtf16String::from("other")));
}

#[test]
fn cross_type_comparison_with_str() {
    let hello = Wtf16String::from("hello");
    assert_eq!(hello, "hello");
    assert!(hello != "world");
    // A borrowed slice compares against a `str` too.
    let owned = Wtf16String::from("hi");
    assert!(&*owned == "hi");
    // Ill-formed content never equals any (well-formed) `str`.
    let ill = Wtf16String::from_units(&[0xD800]);
    assert!(ill != "x");
}
