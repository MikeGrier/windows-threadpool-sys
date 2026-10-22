// Copyright (c) 2026 Mike Grier
//! Tests for `compare_string_ordinal` and what it is made of.

use std::cmp::Ordering;

use win_error_codes::Win32Error;
use windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER;
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN};

use super::{
    Case, CompareStringOrdinalError, Operand, call, compare_string_ordinal, counts, ordering_of,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn compare(first: &[u16], second: &[u16], case: Case) -> Ordering {
    compare_string_ordinal(first, second, case).expect("a comparison")
}

fn compare_text(first: &str, second: &str, case: Case) -> Ordering {
    compare(&wide(first), &wide(second), case)
}

#[test]
fn equal_strings_are_equal_in_both_cases() {
    for case in [Case::Sensitive, Case::Insensitive] {
        for text in ["", "a", "abc", "C:\\Users\\name", "caf\u{e9}", "\u{1f600}"] {
            assert_eq!(compare_text(text, text, case), Ordering::Equal, "{text:?}");
        }
    }
}

#[test]
fn a_smaller_string_is_less_and_the_larger_is_greater() {
    for (smaller, larger) in [
        ("a", "b"),
        ("abc", "abd"),
        ("A", "B"),
        ("0", "9"),
        ("x", "xy"),
    ] {
        assert_eq!(
            compare_text(smaller, larger, Case::Sensitive),
            Ordering::Less,
            "{smaller:?} against {larger:?}"
        );
        assert_eq!(
            compare_text(larger, smaller, Case::Sensitive),
            Ordering::Greater,
            "{larger:?} against {smaller:?}"
        );
    }
}

#[test]
fn a_prefix_is_less_than_what_it_is_a_prefix_of() {
    assert_eq!(compare_text("ab", "abc", Case::Sensitive), Ordering::Less);
    assert_eq!(
        compare_text("abc", "ab", Case::Sensitive),
        Ordering::Greater
    );
    assert_eq!(compare_text("ab", "abc", Case::Insensitive), Ordering::Less);
}

#[test]
fn case_sensitive_comparison_takes_the_units_as_they_are() {
    // 'a' is 0x61 and 'A' is 0x41.
    assert_eq!(
        compare_text("abc", "ABC", Case::Sensitive),
        Ordering::Greater
    );
    assert_eq!(compare_text("ABC", "abc", Case::Sensitive), Ordering::Less);
}

#[test]
fn case_insensitive_comparison_folds_ascii_case() {
    assert_eq!(
        compare_text("abc", "ABC", Case::Insensitive),
        Ordering::Equal
    );
    assert_eq!(
        compare_text("AbC", "aBc", Case::Insensitive),
        Ordering::Equal
    );
}

#[test]
fn case_insensitive_order_is_that_of_the_uppercase_table() {
    // 'a' folds to 'A', which is before 'B', where the units 0x61 and 0x42 are the other way round.
    assert_eq!(compare_text("a", "B", Case::Insensitive), Ordering::Less);
    assert_eq!(compare_text("a", "B", Case::Sensitive), Ordering::Greater);
}

#[test]
fn case_insensitive_comparison_folds_beyond_ascii() {
    for (lower, upper) in [
        ("\u{e9}", "\u{c9}"),
        ("\u{3c3}", "\u{3a3}"),
        ("\u{44f}", "\u{42f}"),
    ] {
        assert_eq!(
            compare_text(lower, upper, Case::Insensitive),
            Ordering::Equal,
            "{lower:?}"
        );
        assert_ne!(
            compare_text(lower, upper, Case::Sensitive),
            Ordering::Equal,
            "{lower:?}"
        );
    }
}

#[test]
fn the_comparison_is_ordinal_and_not_linguistic() {
    // Evidence, not contract: a linguistic comparison could equate these two, and the page says this
    // call tests "binary equality, not linguistic equality".
    assert_ne!(
        compare_text("\u{df}", "ss", Case::Insensitive),
        Ordering::Equal
    );
    assert_ne!(
        compare_text("e\u{301}", "\u{e9}", Case::Insensitive),
        Ordering::Equal
    );
}

#[test]
fn empty_strings_compare_as_the_shortest() {
    for case in [Case::Sensitive, Case::Insensitive] {
        assert_eq!(compare(&[], &[], case), Ordering::Equal);
        assert_eq!(compare(&[], &wide("a"), case), Ordering::Less);
        assert_eq!(compare(&wide("a"), &[], case), Ordering::Greater);
    }
}

#[test]
fn an_embedded_nul_is_a_unit_and_not_the_end() {
    // Passed with the page's -1 these would each be the one-unit string "a".
    let with_nul_b = [u16::from(b'a'), 0, u16::from(b'b')];
    let with_nul_c = [u16::from(b'a'), 0, u16::from(b'c')];
    for case in [Case::Sensitive, Case::Insensitive] {
        assert_eq!(compare(&with_nul_b, &with_nul_c, case), Ordering::Less);
        assert_eq!(compare(&with_nul_c, &with_nul_b, case), Ordering::Greater);
        assert_eq!(compare(&with_nul_b, &with_nul_b, case), Ordering::Equal);
        assert_eq!(
            compare(&[u16::from(b'a'), 0], &[u16::from(b'a')], case),
            Ordering::Greater
        );
        assert_eq!(compare(&[0], &[], case), Ordering::Greater);
    }
}

#[test]
fn a_slice_is_compared_as_the_units_it_holds_and_no_further() {
    let first = wide("abcX");
    let second = wide("abcY");
    assert_eq!(
        compare(&first[..3], &second[..3], Case::Sensitive),
        Ordering::Equal
    );
    assert_eq!(compare(&first, &second, Case::Sensitive), Ordering::Less);
    assert_eq!(
        compare(&first[1..3], &second[..3], Case::Sensitive),
        Ordering::Greater
    );
}

#[test]
fn unpaired_surrogates_are_units() {
    for case in [Case::Sensitive, Case::Insensitive] {
        assert_eq!(compare(&[0xD800], &[0xD800], case), Ordering::Equal);
        assert_eq!(compare(&[0xD800], &[0xDC00], case), Ordering::Less);
        assert_eq!(compare(&[0xDC00], &[0xD800], case), Ordering::Greater);
        assert_eq!(compare(&[0xD800], &[], case), Ordering::Greater);
    }
}

#[test]
fn the_order_observed_is_that_of_the_units_and_not_of_code_points() {
    // Evidence, not contract: the page says only "binary". U+1F600 is the units D83D DE00, and U+FF5E
    // is the one unit FF5E, so by unit order the first is less and by code point order it is greater.
    let emoji = wide("\u{1f600}");
    let fullwidth_tilde = wide("\u{ff5e}");
    assert_eq!(emoji, [0xD83D, 0xDE00]);
    assert_eq!(
        compare(&emoji, &fullwidth_tilde, Case::Sensitive),
        Ordering::Less
    );
}

#[test]
fn a_difference_in_the_last_unit_of_a_long_string_is_found() {
    let mut first = vec![u16::from(b'x'); 1 << 20];
    let mut second = first.clone();
    assert_eq!(compare(&first, &second, Case::Sensitive), Ordering::Equal);
    *first.last_mut().expect("not empty") = u16::from(b'a');
    *second.last_mut().expect("not empty") = u16::from(b'b');
    assert_eq!(compare(&first, &second, Case::Sensitive), Ordering::Less);
}

#[test]
fn swapping_the_strings_reverses_the_ordering_and_equality_is_reflexive() {
    let corpus = [
        "",
        "a",
        "A",
        "b",
        "ab",
        "AB",
        "abc",
        "\u{e9}",
        "\u{c9}",
        "z",
        "Z",
        "\u{1f600}",
        "a\0",
    ];
    for case in [Case::Sensitive, Case::Insensitive] {
        for first in corpus {
            assert_eq!(compare_text(first, first, case), Ordering::Equal);
            for second in corpus {
                assert_eq!(
                    compare_text(first, second, case),
                    compare_text(second, first, case).reverse(),
                    "{first:?} and {second:?} under {case:?}"
                );
            }
        }
    }
}

#[test]
fn a_case_sensitive_comparison_is_equal_exactly_when_the_units_are() {
    let corpus = ["", "a", "A", "ab", "ba", "\u{e9}", "\u{c9}", "a\0b", "a\0"];
    for first in corpus {
        for second in corpus {
            assert_eq!(
                compare_text(first, second, Case::Sensitive) == Ordering::Equal,
                first == second,
                "{first:?} and {second:?}"
            );
        }
    }
}

#[test]
fn case_values_are_the_calls_documented_booleans() {
    // The page: 1 is the only numeric value for true that does not give an invalid parameter error.
    assert_eq!(Case::Insensitive.ignore_case(), 1);
    assert_eq!(Case::Sensitive.ignore_case(), 0);
}

#[test]
fn a_length_the_count_holds_is_passed_as_it_is() {
    assert_eq!(counts(0, 0).expect("counts"), (0, 0));
    assert_eq!(counts(3, 7).expect("counts"), (3, 7));
    let largest = i32::MAX as usize;
    assert_eq!(
        counts(largest, largest).expect("counts"),
        (i32::MAX, i32::MAX)
    );
}

#[test]
fn a_length_the_count_cannot_hold_is_refused_and_names_the_string() {
    let too_long = i32::MAX as usize + 1;
    assert_eq!(
        counts(too_long, 0),
        Err(CompareStringOrdinalError::TooLong {
            operand: Operand::First,
            len: too_long
        })
    );
    assert_eq!(
        counts(0, too_long),
        Err(CompareStringOrdinalError::TooLong {
            operand: Operand::Second,
            len: too_long
        })
    );
    assert_eq!(
        counts(usize::MAX, 1),
        Err(CompareStringOrdinalError::TooLong {
            operand: Operand::First,
            len: usize::MAX
        })
    );
}

#[test]
fn when_both_lengths_are_too_long_the_first_is_named() {
    let first = i32::MAX as usize + 1;
    let second = i32::MAX as usize + 5;
    assert_eq!(
        counts(first, second),
        Err(CompareStringOrdinalError::TooLong {
            operand: Operand::First,
            len: first
        })
    );
}

#[test]
fn the_three_results_the_page_lists_are_the_three_orderings() {
    let never = || -> Win32Error { panic!("the last error is read only for a failure") };
    assert_eq!(ordering_of(CSTR_LESS_THAN, never), Ok(Ordering::Less));
    assert_eq!(ordering_of(CSTR_EQUAL, never), Ok(Ordering::Equal));
    assert_eq!(ordering_of(CSTR_GREATER_THAN, never), Ok(Ordering::Greater));
}

#[test]
fn a_zero_result_is_the_failure_and_reads_the_last_error() {
    let code = Win32Error::from_code(ERROR_INVALID_PARAMETER);
    assert_eq!(ordering_of(0, || code), Err(code));
}

#[test]
fn a_result_the_page_does_not_list_is_a_failure_with_no_code() {
    let never = || -> Win32Error { panic!("the call did not report a failure") };
    for result in [4, -1, 100, i32::MIN, i32::MAX] {
        assert_eq!(
            ordering_of(result, never),
            Err(Win32Error::from_code(0)),
            "{result}"
        );
    }
}

#[test]
fn the_call_failing_gives_its_code() {
    // The failure the page documents: a value of `bIgnoreCase` other than 0 and 1 is "an invalid
    // parameter error", and the page lists ERROR_INVALID_PARAMETER for it. The safe function cannot
    // pass one, so the call is made here directly.
    let text = wide("abc");
    // SAFETY: the pointers are those of `text`, valid for its three units, with those counts.
    let result = unsafe { call(text.as_ptr(), 3, text.as_ptr(), 3, 2) };
    assert_eq!(result, Err(Win32Error::from_code(ERROR_INVALID_PARAMETER)));
}

#[test]
fn a_failure_is_the_calls_and_a_refusal_is_the_crates() {
    let call_failure =
        CompareStringOrdinalError::Call(Win32Error::from_code(ERROR_INVALID_PARAMETER));
    let refusal = CompareStringOrdinalError::TooLong {
        operand: Operand::Second,
        len: 1 << 40,
    };
    assert_ne!(call_failure, refusal);
    assert!(
        std::error::Error::source(&call_failure).is_some(),
        "the call's failure has its code as a source"
    );
    assert!(std::error::Error::source(&refusal).is_none());
    assert!(refusal.to_string().contains("second"));
    assert!(refusal.to_string().contains(&(1u64 << 40).to_string()));
    assert!(call_failure.to_string().contains("CompareStringOrdinal"));
}
