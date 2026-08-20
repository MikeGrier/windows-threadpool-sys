// Copyright (c) 2026 Mike Grier
// Matrix coverage for the WTF-16 string types: a shared corpus of inputs
// (well-formed, ill-formed, and boundary values) with property tests that assert
// each behavior across the whole corpus. `std` (`String::from_utf16[_lossy]`) is
// the oracle wherever the documented contract (D-8) delegates to it.
use crate::{Wtf16, Wtf16Str, Wtf16String, WtfEncoding};

// The encoding's named terminator (mirrors the sibling `tests` module).
const NUL: u16 = Wtf16::NUL;

/// Encode a UTF-8 `str` to WTF-16 code units.
fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// A stable, deterministic hash of a value, for Hash-consistency assertions.
fn hash_of<T: std::hash::Hash + ?Sized>(t: &T) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// The coverage corpus: `(name, content units)`, each entry unique. Mixes
/// well-formed content (empty, ASCII, controls, interior/edge NUL, BMP, astral,
/// scalar boundaries) with ill-formed content (lone/mispaired surrogates in every
/// position). Well-formedness is derived per test via `String::from_utf16`, so it
/// can never drift from a hand-maintained flag.
fn corpus() -> Vec<(&'static str, Vec<u16>)> {
    vec![
        // --- well-formed ---
        ("empty", Vec::new()),
        ("one_ascii", vec![0x41]),
        ("ascii_word", w("hello")),
        ("ascii_apostrophe", w("don't")),
        ("ascii_controls", vec![0x09, 0x0A, 0x0D]),
        ("nul_only", vec![0x00]),
        ("interior_nul", vec![0x61, 0x00, 0x62]),
        ("trailing_content_nul", vec![0x61, 0x00]),
        ("leading_content_nul", vec![0x00, 0x61]),
        ("double_interior_nul", vec![0x61, 0x00, 0x00, 0x62]),
        ("bmp_latin1", w("café")),
        ("bmp_cjk", w("日本語")),
        ("bmp_greek", w("Ωμέγα")),
        ("astral_emoji", w("😀")),
        ("astral_min", vec![0xD800, 0xDC00]), // U+10000
        ("astral_max", vec![0xDBFF, 0xDFFF]), // U+10FFFF
        ("before_surrogates", vec![0xD7FF]),  // U+D7FF
        ("after_surrogates", vec![0xE000]),   // U+E000
        ("bmp_max_scalar", vec![0xFFFF]),     // U+FFFF (valid noncharacter)
        ("replacement_char", vec![0xFFFD]),   // a genuine U+FFFD in content
        ("mixed_planes", w("aé日😀z")),
        ("whitespace_edges", w("  x  ")),
        ("repeated_ascii", vec![0x41; 64]),
        // --- ill-formed (a surrogate is unpaired in some position) ---
        ("lone_high", vec![0xD800]),
        ("lone_high_max", vec![0xDBFF]),
        ("lone_low", vec![0xDC00]),
        ("lone_low_max", vec![0xDFFF]),
        ("high_then_high", vec![0xD800, 0xD801]),
        ("low_then_low", vec![0xDC00, 0xDC01]),
        ("reversed_pair", vec![0xDC00, 0xD800]),
        ("high_then_bmp", vec![0xD800, 0x41]),
        ("bmp_then_low", vec![0x41, 0xDC00]),
        ("high_at_end", vec![0x41, 0x42, 0xD800]),
        ("low_at_start", vec![0xDC00, 0x41, 0x42]),
        ("surrogate_between_valid", vec![0x61, 0xD800, 0x62]),
        ("valid_pair_then_lone_high", vec![0xD800, 0xDC00, 0xD800]),
        ("nul_then_lone_low", vec![0x00, 0xDC00]),
    ]
}

/// Representative well-formed `&str` inputs for the `From<&str>` / `From<String>`
/// direction (every `str` is valid UTF-8, so this side never fails).
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
        let owned = Wtf16String::from_units(&u);
        let mut expected = u.clone();
        expected.push(NUL);
        assert_eq!(owned.units, expected, "{name} buffer");
        assert_eq!(owned.as_units(), u.as_slice(), "{name} content");
        assert_eq!(owned.len(), u.len(), "{name} len");
        assert_eq!(owned.is_empty(), u.is_empty(), "{name} is_empty");
        assert_eq!(*owned.units.last().unwrap(), NUL, "{name} terminator");
        let via_as_ref: &Wtf16Str = owned.as_ref();
        assert_eq!(via_as_ref.as_units(), u.as_slice(), "{name} as_ref");
        assert_eq!((*owned).as_units(), u.as_slice(), "{name} deref");
    }
}

#[test]
fn matrix_borrowed_wraps_without_copying() {
    for (name, u) in corpus() {
        let borrowed = Wtf16Str::from_units(&u);
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
            Wtf16String::from_units(&u).has_interior_nul(),
            contains,
            "{name} owned"
        );
        assert_eq!(
            Wtf16Str::from_units(&u).has_interior_nul(),
            contains,
            "{name} borrowed"
        );
    }
}

#[test]
fn matrix_clone_and_to_owned_preserve_everything() {
    for (name, u) in corpus() {
        let owned = Wtf16String::from_units(&u);
        let cloned = owned.clone();
        assert_eq!(cloned, owned, "{name} clone eq");
        assert_eq!(cloned.units, owned.units, "{name} clone units");
        let borrowed: &Wtf16Str = &owned;
        let reowned = borrowed.to_owned();
        assert_eq!(reowned, owned, "{name} to_owned eq");
        assert_eq!(reowned.units, owned.units, "{name} to_owned units");
    }
}

#[test]
fn matrix_checked_and_lossy_and_display_match_std() {
    for (name, u) in corpus() {
        let owned = Wtf16String::from_units(&u);
        assert_eq!(
            owned.to_string_checked(),
            String::from_utf16(&u).ok(),
            "checked {name}"
        );
        assert_eq!(
            owned.to_string_lossy(),
            String::from_utf16_lossy(&u),
            "lossy {name}"
        );
        assert_eq!(
            format!("{owned}"),
            String::from_utf16_lossy(&u),
            "display {name}"
        );
    }
}

#[test]
fn matrix_into_string_agrees_with_checked() {
    for (name, u) in corpus() {
        let owned = Wtf16String::from_units(&u);
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
        if let Ok(s) = String::from_utf16(&u) {
            let owned = Wtf16String::from_units(&u);
            assert_eq!(format!("{owned:?}"), format!("{s:?}"), "{name}");
        }
    }
}

#[test]
fn matrix_debug_escapes_ill_formed_and_is_quoted() {
    for (name, u) in corpus() {
        let d = format!("{:?}", Wtf16String::from_units(&u));
        assert!(d.starts_with('"') && d.ends_with('"'), "{name}: {d}");
        assert!(
            !d.contains('\n') && !d.contains('\r'),
            "{name} raw control: {d}"
        );
        if String::from_utf16(&u).is_err() {
            assert!(
                d.contains("\\u{"),
                "{name}: expected a lossless escape in {d}"
            );
        }
    }
}

#[test]
fn matrix_eq_str_reflexive_for_wellformed() {
    for (name, u) in corpus() {
        if let Ok(s) = String::from_utf16(&u) {
            let owned = Wtf16String::from_units(&u);
            assert!(owned == s.as_str(), "{name} owned eq str");
            let borrowed = Wtf16Str::from_units(&u);
            assert!(*borrowed == *s.as_str(), "{name} borrowed eq str");
        }
    }
}

#[test]
fn matrix_ill_formed_never_equals_its_lossy_str() {
    // A lossy view substitutes U+FFFD for surrogates, so re-encoding it differs
    // from the original units: equality is exact over units, not over the view.
    for (name, u) in corpus() {
        if String::from_utf16(&u).is_err() {
            let owned = Wtf16String::from_units(&u);
            let lossy = String::from_utf16_lossy(&u);
            assert!(owned != lossy.as_str(), "{name}");
        }
    }
}

#[test]
fn matrix_eq_and_hash_track_content_units() {
    let entries = corpus();
    for (na, ua) in &entries {
        let a = Wtf16String::from_units(ua);
        for (nb, ub) in &entries {
            let b = Wtf16String::from_units(ub);
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
        let owned = Wtf16String::from_units(&u);
        let borrowed: &Wtf16Str = &owned;
        assert_eq!(hash_of(&owned), hash_of(borrowed), "{name}");
    }
}

#[test]
fn matrix_hashmap_lookup_by_borrowed_slice() {
    use std::collections::HashMap;
    let entries = corpus();
    let mut map: HashMap<Wtf16String, usize> = HashMap::new();
    for (i, (_, u)) in entries.iter().enumerate() {
        map.insert(Wtf16String::from_units(u), i);
    }
    for (i, (name, u)) in entries.iter().enumerate() {
        let owned = Wtf16String::from_units(u);
        let borrowed: &Wtf16Str = &owned;
        assert_eq!(map.get(borrowed).copied(), Some(i), "{name}");
    }
}

#[test]
fn matrix_ordering_is_lexicographic_over_content_units() {
    let mut owned: Vec<Wtf16String> = corpus()
        .into_iter()
        .map(|(_, u)| Wtf16String::from_units(&u))
        .collect();
    owned.sort();
    let got: Vec<Vec<u16>> = owned.iter().map(|s| s.as_units().to_vec()).collect();
    let mut expected = got.clone();
    expected.sort();
    assert_eq!(
        got, expected,
        "Ord must equal lexicographic content-unit order"
    );
}

#[test]
fn matrix_counted_ptr_and_from_wide_ptr_roundtrip() {
    for (name, u) in corpus() {
        let owned = Wtf16String::from_units(&u);
        // SAFETY: `as_ptr` is valid for `len()` content reads.
        let seen = unsafe { std::slice::from_raw_parts(owned.as_ptr(), owned.len()) };
        assert_eq!(seen, u.as_slice(), "as_ptr {name}");
        // SAFETY: same region, copied by value; no reference retained.
        let copy = unsafe { Wtf16String::from_wide_ptr(owned.as_ptr(), owned.len()) };
        assert_eq!(copy.as_units(), u.as_slice(), "from_wide_ptr {name}");
        assert_eq!(copy, owned, "{name} round-trip eq");
    }
}

#[test]
fn matrix_terminated_ptr_stops_at_first_nul() {
    for (name, u) in corpus() {
        let owned = Wtf16String::from_units(&u);
        let ptr = owned.as_terminated_ptr();
        let mut read = Vec::new();
        let mut i = 0isize;
        loop {
            // SAFETY: the buffer is `[content.., NUL]`, so a forward scan reaches
            // a NUL within bounds (an interior NUL, or the terminator).
            let unit = unsafe { *ptr.offset(i) };
            if unit == NUL {
                break;
            }
            read.push(unit);
            i += 1;
        }
        let first_nul = u.iter().position(|&x| x == NUL).unwrap_or(u.len());
        assert_eq!(read.as_slice(), &u[..first_nul], "{name}");
    }
}

#[test]
fn matrix_buffer_fill_excludes_nul_reconstructs() {
    for (name, u) in corpus() {
        let mut buf = Wtf16String::with_capacity(u.len());
        // SAFETY: write `u.len()` content units, publish that many; within capacity.
        unsafe {
            std::ptr::copy_nonoverlapping(u.as_ptr(), buf.as_mut_ptr(), u.len());
            buf.set_len_from_ffi(u.len());
        }
        assert_eq!(buf.as_units(), u.as_slice(), "{name}");
        assert_eq!(*buf.units.last().unwrap(), NUL, "{name} terminator");
    }
}

#[test]
fn matrix_buffer_fill_includes_nul_reconstructs() {
    for (name, u) in corpus() {
        let mut src = u.clone();
        src.push(NUL); // the callee writes its own terminator into the reserved slot
        let mut buf = Wtf16String::with_capacity(u.len());
        // SAFETY: `with_capacity(u.len())` reserves `u.len() + 1`, so writing
        // `src.len()` units stays in bounds; publishing `u.len()` keeps content only.
        unsafe {
            std::ptr::copy_nonoverlapping(src.as_ptr(), buf.as_mut_ptr(), src.len());
            buf.set_len_from_ffi(u.len());
        }
        assert_eq!(buf.as_units(), u.as_slice(), "{name}");
    }
}

#[test]
fn matrix_from_str_and_from_string_encode_and_round_trip() {
    for s in well_formed_strs() {
        let owned = Wtf16String::from(s);
        let expected = w(s);
        assert_eq!(owned.as_units(), expected.as_slice(), "units {s:?}");
        assert_eq!(
            owned.to_string_checked().as_deref(),
            Some(s),
            "checked {s:?}"
        );
        assert_eq!(owned.to_string_lossy(), s, "lossy {s:?}");
        assert_eq!(owned.clone().into_string().unwrap(), s, "into_string {s:?}");
        let from_string = Wtf16String::from(s.to_string());
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
        let owned = Wtf16String::from(s.as_str());
        assert_eq!(owned.as_units(), w(&s).as_slice(), "U+{cp:04X} units");
        assert_eq!(
            owned.to_string_checked().as_deref(),
            Some(s.as_str()),
            "U+{cp:04X} checked"
        );
    }
}

#[test]
fn surrogate_range_units_are_ill_formed_alone_neighbours_are_valid() {
    for unit in [0xD800u16, 0xDABF, 0xDBFF, 0xDC00, 0xDDDD, 0xDFFF] {
        let arr = [unit];
        let s = Wtf16Str::from_units(&arr);
        assert_eq!(s.to_string_checked(), None, "unit {unit:#06x} checked");
        assert_eq!(s.to_string_lossy(), "\u{FFFD}", "unit {unit:#06x} lossy");
    }
    for unit in [0xD7FFu16, 0xE000] {
        assert!(
            Wtf16Str::from_units(&[unit]).to_string_checked().is_some(),
            "unit {unit:#06x} should be a valid scalar"
        );
    }
}

#[test]
fn every_surrogate_pairing_decodes_to_the_expected_astral_scalar() {
    for &hi in &[0xD800u16, 0xD801, 0xDBFF] {
        for &lo in &[0xDC00u16, 0xDC01, 0xDFFF] {
            let owned = Wtf16String::from_units(&[hi, lo]);
            let expected = 0x1_0000 + (((hi as u32 - 0xD800) << 10) | (lo as u32 - 0xDC00));
            let ch = char::from_u32(expected).expect("valid astral scalar");
            assert_eq!(
                owned.to_string_checked().as_deref(),
                Some(ch.to_string().as_str()),
                "{hi:#06x},{lo:#06x}"
            );
            assert_eq!(owned.len(), 2, "{hi:#06x},{lo:#06x} two units");
        }
    }
}

#[test]
fn push_appends_corpus_content_and_reestablishes_terminator() {
    for (name, u) in corpus() {
        let mut owned = Wtf16String::new();
        owned.push(Wtf16Str::from_units(&u));
        assert_eq!(owned.as_units(), u.as_slice(), "{name}");
        assert_eq!(*owned.units.last().unwrap(), NUL, "{name} terminator");
    }
}

#[test]
fn push_concatenates_two_corpus_entries() {
    let entries = corpus();
    for (na, ua) in &entries {
        for (nb, ub) in &entries {
            let mut owned = Wtf16String::from_units(ua);
            owned.push(Wtf16Str::from_units(ub));
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
    let mut owned = Wtf16String::from_units(&[0x61, NUL, 0x62]); // "a\0b"
    owned.push(Wtf16Str::from_units(&[0x63])); // + "c"
    assert_eq!(owned.as_units(), [0x61, NUL, 0x62, 0x63]);
    assert!(owned.has_interior_nul());
}

#[test]
fn push_str_encodes_and_appends() {
    for s in well_formed_strs() {
        let mut owned = Wtf16String::from(s);
        owned.push_str(s);
        let mut expected = w(s);
        expected.extend_from_slice(&w(s));
        assert_eq!(owned.as_units(), expected.as_slice(), "{s:?}");
    }
}

#[test]
fn clear_empties_content_and_round_trips_through_push() {
    for (name, u) in corpus() {
        let mut owned = Wtf16String::from_units(&u);
        owned.clear();
        assert!(owned.is_empty(), "{name} cleared");
        assert_eq!(owned.units, vec![NUL], "{name} terminator-only buffer");
        owned.push(Wtf16Str::from_units(&u));
        assert_eq!(
            owned.as_units(),
            u.as_slice(),
            "{name} round trip after clear"
        );
    }
}

#[test]
fn capacity_reserve_and_shrink_behave_as_documented() {
    let mut owned = Wtf16String::new();
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
    assert_eq!(owned.as_units(), w("hello").as_slice());
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
