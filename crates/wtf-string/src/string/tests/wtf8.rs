// Copyright (c) 2026 Mike Grier
// Matrix coverage for the WTF-8 string types: a shared corpus of `u8` inputs
// (well-formed UTF-8, WTF-8-encoded surrogates, and arbitrary invalid bytes) with
// property tests that assert each behavior across the whole corpus. `std`
// (`str::from_utf8` / `String::from_utf8_lossy`) is the oracle wherever the
// documented contract delegates to it. Cross-width parity tests pin the shared,
// encoding-generic `str`-level semantics to the `Wtf16` arm.
use crate::{Wtf8, Wtf8Str, Wtf8String, Wtf16String, WtfEncoding};

// The encoding's named terminator (mirrors the sibling `tests` module).
const NUL: u8 = Wtf8::NUL;

/// Encode a UTF-8 `str` to WTF-8 code units (the identity on its bytes).
fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// WTF-8 encoding of a code point, including the surrogate range `D800..=DFFF`
/// that `char` cannot represent. Surrogates occupy three bytes like any other
/// `U+0800..=U+FFFF` scalar; the result is valid WTF-8 but invalid UTF-8.
fn wtf8_of(cp: u32) -> Vec<u8> {
    if cp < 0x80 {
        vec![cp as u8]
    } else if cp < 0x800 {
        vec![0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8]
    } else if cp < 0x1_0000 {
        vec![
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]
    } else {
        vec![
            0xF0 | (cp >> 18) as u8,
            0x80 | ((cp >> 12) & 0x3F) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]
    }
}

/// A stable, deterministic hash of a value, for Hash-consistency assertions.
fn hash_of<T: std::hash::Hash + ?Sized>(t: &T) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// The coverage corpus: `(name, content bytes)`, each entry unique. Mixes
/// well-formed UTF-8 (empty, ASCII, controls, interior/edge NUL, multi-byte
/// scalars, scalar boundaries) with ill-formed WTF-8 (encoded surrogates in every
/// position) and arbitrary invalid bytes. Well-formedness is derived per test via
/// `str::from_utf8`, so it can never drift from a hand-maintained flag.
fn corpus() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        // --- well-formed UTF-8 (also valid WTF-8) ---
        ("empty", Vec::new()),
        ("one_ascii", vec![0x41]),
        ("ascii_word", b("hello")),
        ("ascii_apostrophe", b("don't")),
        ("ascii_controls", vec![0x09, 0x0A, 0x0D]),
        ("nul_only", vec![0x00]),
        ("interior_nul", vec![0x61, 0x00, 0x62]),
        ("trailing_content_nul", vec![0x61, 0x00]),
        ("leading_content_nul", vec![0x00, 0x61]),
        ("double_interior_nul", vec![0x61, 0x00, 0x00, 0x62]),
        ("two_byte_latin1", b("café")),
        ("three_byte_cjk", b("日本語")),
        ("three_byte_greek", b("Ωμέγα")),
        ("four_byte_emoji", b("😀")),
        ("astral_min", b("\u{10000}")),
        ("astral_max", b("\u{10FFFF}")),
        ("before_surrogates", b("\u{D7FF}")),
        ("after_surrogates", b("\u{E000}")),
        ("bmp_max_scalar", b("\u{FFFF}")),
        ("replacement_char", b("\u{FFFD}")),
        ("mixed_widths", b("aé日😀z")),
        ("whitespace_edges", b("  x  ")),
        ("repeated_ascii", vec![0x41; 64]),
        // --- ill-formed WTF-8: encoded lone/mispaired surrogates (valid WTF-8,
        //     invalid UTF-8, always three bytes) ---
        ("lone_high", wtf8_of(0xD800)),
        ("lone_high_max", wtf8_of(0xDBFF)),
        ("lone_low", wtf8_of(0xDC00)),
        ("lone_low_max", wtf8_of(0xDFFF)),
        ("surrogate_between_valid", {
            let mut v = b("a");
            v.extend(wtf8_of(0xD800));
            v.extend(b("b"));
            v
        }),
        ("nul_then_surrogate", {
            let mut v = vec![0x00];
            v.extend(wtf8_of(0xDC00));
            v
        }),
        // --- arbitrary invalid bytes: not valid WTF-8 at all, yet still stored
        //     verbatim; decode/lossy fall back to the std oracle ---
        ("lone_continuation", vec![0x80]),
        ("invalid_ff", vec![0xFF]),
        ("invalid_fe", vec![0xFE]),
        ("truncated_two_byte", vec![0xC3]),
        ("truncated_three_byte", vec![0xE0, 0xA0]),
        ("truncated_four_byte", vec![0xF0, 0x9F, 0x98]),
        ("overlong_nul", vec![0xC0, 0x80]),
        ("continuation_run", vec![0x80, 0x80, 0x80]),
        ("bad_then_ascii", vec![0xFF, 0x41]),
        ("ascii_then_bad", vec![0x41, 0xFF]),
    ]
}

/// Representative well-formed `&str` inputs for the `From<&str>` / `From<String>`
/// direction (every `str` is valid UTF-8, so this side never fails). Shared with
/// the `Wtf16` matrix so cross-width parity compares the same inputs.
fn well_formed_strs() -> Vec<&'static str> {
    vec![
        "",
        "a",
        "ascii sentence with spaces",
        "don't \"quote\" me",
        "tab\tnewline\nreturn\r",
        "null\u{0}inside",
        "café",
        "naïve",
        "Ωμέγα",
        "日本語のテキスト",
        "😀🎉🚀",
        "aé日😀z",
        "\u{7F}\u{80}\u{7FF}\u{800}\u{D7FF}\u{E000}\u{FFFF}",
        "\u{10000}\u{10FFFF}",
    ]
}

#[test]
fn matrix_storage_invariant_holds() {
    for (name, u) in corpus() {
        let owned = Wtf8String::from_units(&u);
        let mut expected = u.clone();
        expected.push(NUL);
        assert_eq!(owned.units, expected, "{name} buffer");
        assert_eq!(owned.as_units(), u.as_slice(), "{name} content");
        assert_eq!(owned.len(), u.len(), "{name} len");
        assert_eq!(owned.is_empty(), u.is_empty(), "{name} is_empty");
        assert_eq!(*owned.units.last().unwrap(), NUL, "{name} terminator");
        let via_as_ref: &Wtf8Str = owned.as_ref();
        assert_eq!(via_as_ref.as_units(), u.as_slice(), "{name} as_ref");
        assert_eq!((*owned).as_units(), u.as_slice(), "{name} deref");
    }
}

#[test]
fn matrix_borrowed_wraps_without_copying() {
    for (name, u) in corpus() {
        let borrowed = Wtf8Str::from_units(&u);
        assert_eq!(borrowed.as_units(), u.as_slice(), "{name}");
        assert_eq!(borrowed.len(), u.len(), "{name}");
        assert_eq!(borrowed.as_units().as_ptr(), u.as_ptr(), "{name} zero-copy");
    }
}

#[test]
fn matrix_has_interior_nul_matches_content() {
    for (name, u) in corpus() {
        let contains = u.contains(&NUL);
        assert_eq!(
            Wtf8String::from_units(&u).has_interior_nul(),
            contains,
            "{name} owned"
        );
        assert_eq!(
            Wtf8Str::from_units(&u).has_interior_nul(),
            contains,
            "{name} borrowed"
        );
    }
}

#[test]
fn matrix_clone_and_to_owned_preserve_everything() {
    for (name, u) in corpus() {
        let owned = Wtf8String::from_units(&u);
        let cloned = owned.clone();
        assert_eq!(cloned, owned, "{name} clone eq");
        assert_eq!(cloned.units, owned.units, "{name} clone units");
        let borrowed: &Wtf8Str = &owned;
        let reowned = borrowed.to_owned();
        assert_eq!(reowned, owned, "{name} to_owned eq");
        assert_eq!(reowned.units, owned.units, "{name} to_owned units");
    }
}

#[test]
fn matrix_checked_and_lossy_and_display_match_std() {
    for (name, u) in corpus() {
        let owned = Wtf8String::from_units(&u);
        assert_eq!(
            owned.to_string_checked(),
            std::str::from_utf8(&u).ok().map(String::from),
            "checked {name}"
        );
        assert_eq!(
            owned.to_string_lossy(),
            String::from_utf8_lossy(&u),
            "lossy {name}"
        );
        assert_eq!(
            format!("{owned}"),
            String::from_utf8_lossy(&u),
            "display {name}"
        );
    }
}

#[test]
fn matrix_into_string_agrees_with_checked() {
    for (name, u) in corpus() {
        let owned = Wtf8String::from_units(&u);
        let checked = owned.to_string_checked();
        match owned.into_string() {
            Ok(s) => assert_eq!(Some(s), checked, "{name} ok matches checked"),
            Err(orig) => {
                assert_eq!(checked, None, "{name} err implies checked None");
                assert_eq!(orig.as_units(), u.as_slice(), "{name} original preserved");
            }
        }
    }
}

#[test]
fn matrix_debug_matches_string_debug_for_wellformed() {
    // For well-formed content, string-style Debug must equal std `String` Debug
    // (a non-circular oracle: quoting, control escapes, literal apostrophe).
    for (name, u) in corpus() {
        if let Ok(s) = std::str::from_utf8(&u) {
            let owned = Wtf8String::from_units(&u);
            assert_eq!(format!("{owned:?}"), format!("{s:?}"), "{name}");
        }
    }
}

#[test]
fn matrix_debug_escapes_ill_formed_and_is_quoted() {
    for (name, u) in corpus() {
        let d = format!("{:?}", Wtf8String::from_units(&u));
        assert!(d.starts_with('"') && d.ends_with('"'), "{name}: {d}");
        assert!(
            !d.contains('\n') && !d.contains('\r'),
            "{name} raw control: {d}"
        );
        if std::str::from_utf8(&u).is_err() {
            assert!(
                d.contains("\\x"),
                "{name}: expected a lossless byte escape in {d}"
            );
        }
    }
}

#[test]
fn matrix_eq_str_reflexive_for_wellformed() {
    for (name, u) in corpus() {
        if let Ok(s) = std::str::from_utf8(&u) {
            let owned = Wtf8String::from_units(&u);
            assert!(owned == s, "{name} owned eq str");
            let borrowed = Wtf8Str::from_units(&u);
            assert!(*borrowed == *s, "{name} borrowed eq str");
        }
    }
}

#[test]
fn matrix_ill_formed_never_equals_its_lossy_str() {
    // A lossy view substitutes U+FFFD for ill-formed bytes, so re-encoding it
    // differs from the original bytes: equality is exact over units, not the view.
    for (name, u) in corpus() {
        if std::str::from_utf8(&u).is_err() {
            let owned = Wtf8String::from_units(&u);
            let lossy = String::from_utf8_lossy(&u).into_owned();
            assert!(owned != lossy.as_str(), "{name}");
        }
    }
}

#[test]
fn matrix_eq_and_hash_track_content_units() {
    let entries = corpus();
    for (na, ua) in &entries {
        let a = Wtf8String::from_units(ua);
        for (nb, ub) in &entries {
            let b = Wtf8String::from_units(ub);
            let units_eq = ua == ub;
            assert_eq!(a == b, units_eq, "eq {na} vs {nb}");
            if units_eq {
                assert_eq!(hash_of(&a), hash_of(&b), "hash {na} vs {nb}");
            }
        }
    }
}

#[test]
fn matrix_owned_and_borrowed_hash_equal() {
    for (name, u) in corpus() {
        let owned = Wtf8String::from_units(&u);
        let borrowed: &Wtf8Str = &owned;
        assert_eq!(hash_of(&owned), hash_of(borrowed), "{name}");
    }
}

#[test]
fn matrix_hashmap_lookup_by_borrowed_slice() {
    use std::collections::HashMap;
    let entries = corpus();
    let mut map: HashMap<Wtf8String, usize> = HashMap::new();
    for (i, (_, u)) in entries.iter().enumerate() {
        map.insert(Wtf8String::from_units(u), i);
    }
    for (i, (name, u)) in entries.iter().enumerate() {
        let owned = Wtf8String::from_units(u);
        let borrowed: &Wtf8Str = &owned;
        assert_eq!(map.get(borrowed).copied(), Some(i), "{name}");
    }
}

#[test]
fn matrix_ordering_is_lexicographic_over_content_units() {
    let mut owned: Vec<Wtf8String> = corpus()
        .into_iter()
        .map(|(_, u)| Wtf8String::from_units(&u))
        .collect();
    owned.sort();
    let got: Vec<Vec<u8>> = owned.iter().map(|s| s.as_units().to_vec()).collect();
    let mut expected = got.clone();
    expected.sort();
    assert_eq!(
        got, expected,
        "Ord must equal lexicographic content-unit order"
    );
}

#[test]
fn matrix_from_str_and_from_string_encode_and_round_trip() {
    for s in well_formed_strs() {
        let owned = Wtf8String::from(s);
        let expected = b(s);
        assert_eq!(owned.as_units(), expected.as_slice(), "units {s:?}");
        assert_eq!(
            owned.to_string_checked().as_deref(),
            Some(s),
            "checked {s:?}"
        );
        assert_eq!(owned.to_string_lossy(), s, "lossy {s:?}");
        assert_eq!(owned.clone().into_string().unwrap(), s, "into_string {s:?}");
        let from_string = Wtf8String::from(s.to_string());
        assert_eq!(from_string.units, owned.units, "from String {s:?}");
        assert!(owned == s, "eq_str {s:?}");
    }
}

#[test]
fn scalar_boundary_values_round_trip() {
    // Scalars just inside each boundary: plane edges and the surrogate gap edges.
    for cp in [
        0x0000u32, 0x007F, 0x0080, 0x07FF, 0x0800, 0xD7FF, 0xE000, 0xFFFD, 0xFFFF, 0x1_0000,
        0x10_FFFF,
    ] {
        let ch = char::from_u32(cp).expect("valid scalar");
        let s = ch.to_string();
        let owned = Wtf8String::from(s.as_str());
        assert_eq!(owned.as_units(), b(&s).as_slice(), "U+{cp:04X} units");
        assert_eq!(
            owned.to_string_checked().as_deref(),
            Some(s.as_str()),
            "U+{cp:04X} checked"
        );
    }
}

#[test]
fn wtf8_encoded_surrogates_are_ill_formed_and_lossy_replaces() {
    // Every code point in the surrogate range encodes as valid, three-byte WTF-8
    // that a strict `String` rejects; the lossy view is entirely U+FFFD. Because
    // `from_utf8_lossy` is byte-granular, a three-byte encoded surrogate becomes
    // three replacement chars -- we assert against the std oracle, not a count.
    for cp in [0xD800u32, 0xDABF, 0xDBFF, 0xDC00, 0xDDDD, 0xDFFF] {
        let u = wtf8_of(cp);
        assert_eq!(u.len(), 3, "U+{cp:04X} occupies three WTF-8 bytes");
        let owned = Wtf8String::from_units(&u);
        assert_eq!(owned.to_string_checked(), None, "U+{cp:04X} checked");
        let lossy = owned.to_string_lossy();
        assert_eq!(
            lossy,
            String::from_utf8_lossy(&u),
            "U+{cp:04X} lossy oracle"
        );
        assert!(
            !lossy.is_empty() && lossy.chars().all(|c| c == '\u{FFFD}'),
            "U+{cp:04X} lossy is all replacement chars: {lossy:?}"
        );
    }
}

#[test]
fn arbitrary_invalid_bytes_are_stored_verbatim() {
    // Construction performs no validation (D-4): arbitrary bytes survive in
    // storage exactly, even when neither UTF-8 nor WTF-8 could produce them.
    for u in [
        vec![0xFFu8],
        vec![0x80],
        vec![0xC0, 0x80],
        vec![0xFF, 0xFE, 0xFD],
        vec![0x41, 0xFF, 0x42],
    ] {
        let owned = Wtf8String::from_units(&u);
        assert_eq!(owned.as_units(), u.as_slice(), "bytes {u:02x?}");
        assert_eq!(owned.to_string_checked(), None, "checked {u:02x?}");
    }
}

#[test]
fn cross_width_parity_for_well_formed() {
    // The encoding-generic `str`-level API must agree across storage widths for
    // every well-formed input, even though the raw code units differ.
    for s in well_formed_strs() {
        let eight = Wtf8String::from(s);
        let sixteen = Wtf16String::from(s);
        assert_eq!(
            eight.to_string_checked(),
            sixteen.to_string_checked(),
            "checked {s:?}"
        );
        assert_eq!(eight.to_string_checked().as_deref(), Some(s), "value {s:?}");
        assert_eq!(
            eight.to_string_lossy(),
            sixteen.to_string_lossy(),
            "lossy {s:?}"
        );
        assert_eq!(format!("{eight}"), format!("{sixteen}"), "display {s:?}");
        assert_eq!(format!("{eight:?}"), format!("{sixteen:?}"), "debug {s:?}");
        assert_eq!(format!("{eight:?}"), format!("{s:?}"), "debug vs std {s:?}");
        assert!(eight == s && sixteen == s, "eq_str {s:?}");
    }
}

#[test]
fn cross_width_parity_for_ill_formed_surrogates() {
    // A lone surrogate is expressible in both widths -- one `u16` unit for
    // `Wtf16`, three WTF-8 bytes for `Wtf8` -- and both are ill-formed for a
    // strict `String` yet lossily replace with only U+FFFD. The *count* of U+FFFD
    // differs by width (byte-granular vs unit-granular replacement), so parity is
    // over checked-decode failure and full replacement, not raw string equality.
    for cp in [0xD800u32, 0xD801, 0xDABF, 0xDBFF, 0xDC00, 0xDDDD, 0xDFFF] {
        let eight = Wtf8String::from_units(&wtf8_of(cp));
        let sixteen = Wtf16String::from_units(&[cp as u16]);
        assert_eq!(eight.to_string_checked(), None, "U+{cp:04X} wtf8 checked");
        assert_eq!(
            sixteen.to_string_checked(),
            None,
            "U+{cp:04X} wtf16 checked"
        );
        let eight_lossy = eight.to_string_lossy();
        let sixteen_lossy = sixteen.to_string_lossy();
        assert!(
            !eight_lossy.is_empty() && eight_lossy.chars().all(|c| c == '\u{FFFD}'),
            "U+{cp:04X} wtf8 lossy all replacement: {eight_lossy:?}"
        );
        assert!(
            !sixteen_lossy.is_empty() && sixteen_lossy.chars().all(|c| c == '\u{FFFD}'),
            "U+{cp:04X} wtf16 lossy all replacement: {sixteen_lossy:?}"
        );
    }
}

#[test]
fn push_appends_corpus_content_and_reestablishes_terminator() {
    for (name, u) in corpus() {
        let mut owned = Wtf8String::new();
        owned.push(Wtf8Str::from_units(&u));
        assert_eq!(owned.as_units(), u.as_slice(), "{name}");
        assert_eq!(*owned.units.last().unwrap(), NUL, "{name} terminator");
    }
}

#[test]
fn push_concatenates_two_corpus_entries() {
    let entries = corpus();
    for (na, ua) in &entries {
        for (nb, ub) in &entries {
            let mut owned = Wtf8String::from_units(ua);
            owned.push(Wtf8Str::from_units(ub));
            let mut expected = ua.clone();
            expected.extend_from_slice(ub);
            assert_eq!(owned.as_units(), expected.as_slice(), "{na} + {nb}");
            assert_eq!(*owned.units.last().unwrap(), NUL, "{na} + {nb} terminator");
        }
    }
}

#[test]
fn push_preserves_interior_nul_contributed_by_either_side() {
    // Neither side alone has an interior NUL at the join point; concatenation
    // must not lose or misplace either side's own interior NUL.
    let mut owned = Wtf8String::from_units(&[0x61, NUL, 0x62]); // "a\0b"
    owned.push(Wtf8Str::from_units(&[0x63])); // + "c"
    assert_eq!(owned.as_units(), [0x61, NUL, 0x62, 0x63]);
    assert!(owned.has_interior_nul());
}

#[test]
fn push_str_encodes_and_appends() {
    for s in well_formed_strs() {
        let mut owned = Wtf8String::from(s);
        owned.push_str(s);
        let mut expected = b(s);
        expected.extend_from_slice(&b(s));
        assert_eq!(owned.as_units(), expected.as_slice(), "{s:?}");
    }
}

#[test]
fn clear_empties_content_and_round_trips_through_push() {
    for (name, u) in corpus() {
        let mut owned = Wtf8String::from_units(&u);
        owned.clear();
        assert!(owned.is_empty(), "{name} cleared");
        assert_eq!(owned.units, vec![NUL], "{name} terminator-only buffer");
        owned.push(Wtf8Str::from_units(&u));
        assert_eq!(
            owned.as_units(),
            u.as_slice(),
            "{name} round trip after clear"
        );
    }
}

#[test]
fn capacity_reserve_and_shrink_behave_as_documented() {
    let mut owned = Wtf8String::new();
    assert_eq!(
        owned.capacity(),
        0,
        "a fresh string reserves no spare content capacity"
    );

    owned.reserve(64);
    assert!(
        owned.capacity() >= 64,
        "reserve grows content capacity by at least the request"
    );

    owned.push_str("hello");
    assert_eq!(owned.as_units(), b("hello").as_slice());
    assert!(
        owned.capacity() >= owned.len(),
        "capacity never falls below content length"
    );

    owned.reserve_exact(10);
    assert!(owned.capacity() >= owned.len() + 10);

    owned.shrink_to_fit();
    assert!(
        owned.capacity() >= owned.len(),
        "shrink_to_fit never drops below content length"
    );

    owned.reserve(100);
    owned.shrink_to(5);
    assert!(
        owned.capacity() < 100,
        "shrink_to must actually shrink when given a smaller bound"
    );
}
