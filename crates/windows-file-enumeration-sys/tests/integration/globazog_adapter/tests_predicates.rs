// Copyright (c) 2026 Mike Grier
//! Every Globazog predicate leaf translates and evaluates losslessly through
//! the adapter, against real files.

use crate::globazog_adapter::adapter::enumerate_dir_native_via_wfe_with_predicate;
use crate::globazog_adapter::predicate_types::{CaseSensitivity, Cmp, Leaf, TimeField, Token};
use crate::globazog_adapter::tests_support::ascii_name;
use crate::globazog_adapter::types::{EntryType, EnumPlan};
use crate::support::Scratch;

fn literal_segment(text: &str) -> Vec<Token> {
    text.chars().map(|c| Token::Literal(c as u32)).collect()
}

fn run(scratch: &Scratch, leaves: &[Leaf]) -> Vec<String> {
    let scan = enumerate_dir_native_via_wfe_with_predicate(scratch.path(), EnumPlan::FULL, leaves)
        .expect("a scan");
    let mut names: Vec<String> = scan.entries.iter().map(ascii_name).collect();
    names.sort();
    names
}

#[test]
fn name_leaf_matches_an_exact_segment_and_respects_case() {
    let scratch = Scratch::with_files(&["Report.txt"]);

    let sensitive = [Leaf::Name {
        seg: literal_segment("report.txt"),
        case: CaseSensitivity::Sensitive,
        negate: false,
    }];
    assert!(run(&scratch, &sensitive).is_empty());

    let insensitive = [Leaf::Name {
        seg: literal_segment("report.txt"),
        case: CaseSensitivity::Insensitive,
        negate: false,
    }];
    assert_eq!(run(&scratch, &insensitive), ["Report.txt"]);
}

#[test]
fn name_leaf_negated_matches_everything_else() {
    let scratch = Scratch::with_files(&["a.txt", "b.txt"]);
    let leaves = [Leaf::Name {
        seg: literal_segment("a.txt"),
        case: CaseSensitivity::Sensitive,
        negate: true,
    }];
    assert_eq!(run(&scratch, &leaves), ["b.txt"]);
}

#[test]
fn name_in_set_matches_any_alternative() {
    let scratch = Scratch::with_files(&["one.txt", "two.txt", "three.txt"]);
    let leaves = [Leaf::NameInSet {
        segs: vec![literal_segment("one.txt"), literal_segment("three.txt")],
        case: CaseSensitivity::Sensitive,
        negate: false,
    }];
    assert_eq!(run(&scratch, &leaves), ["one.txt", "three.txt"]);
}

#[test]
fn a_wildcard_token_matches_a_run_of_any_length() {
    let scratch = Scratch::with_files(&["report-2026.log", "report.log", "other.txt"]);
    let mut seg = literal_segment("report");
    seg.push(Token::Star);
    seg.extend(literal_segment(".log"));
    let leaves = [Leaf::Name {
        seg,
        case: CaseSensitivity::Sensitive,
        negate: false,
    }];
    assert_eq!(run(&scratch, &leaves), ["report-2026.log", "report.log"]);
}

#[test]
fn an_any_token_matches_exactly_one_code_point() {
    let scratch = Scratch::with_files(&["a1.dat", "a22.dat", "ab.dat"]);
    let mut seg = literal_segment("a");
    seg.push(Token::Any);
    seg.extend(literal_segment(".dat"));
    let leaves = [Leaf::Name {
        seg,
        case: CaseSensitivity::Sensitive,
        negate: false,
    }];
    // `a22.dat` has two code points where the pattern asks for exactly one,
    // so it must not match; `a1.dat` and `ab.dat` each have exactly one.
    assert_eq!(run(&scratch, &leaves), ["a1.dat", "ab.dat"]);
}

#[test]
fn an_alternation_token_matches_any_of_its_arms() {
    let scratch = Scratch::with_files(&["cat.dat", "dog.dat", "fish.dat"]);
    let mut seg = vec![Token::Alt(vec![
        literal_segment("cat"),
        literal_segment("dog"),
    ])];
    seg.extend(literal_segment(".dat"));
    let leaves = [Leaf::Name {
        seg,
        case: CaseSensitivity::Sensitive,
        negate: false,
    }];
    assert_eq!(run(&scratch, &leaves), ["cat.dat", "dog.dat"]);
}

#[test]
fn is_type_distinguishes_files_from_directories() {
    let scratch = Scratch::with_files(&["a-file.txt"]);
    scratch.subdir("a-directory");

    let files = [Leaf::IsType {
        ty: EntryType::File,
        negate: false,
    }];
    assert_eq!(run(&scratch, &files), ["a-file.txt"]);

    let directories = [Leaf::IsType {
        ty: EntryType::Dir,
        negate: false,
    }];
    assert_eq!(run(&scratch, &directories), ["a-directory"]);
}

#[test]
fn is_type_other_never_matches_on_windows_and_negated_always_does() {
    let scratch = Scratch::with_files(&["a-file.txt"]);
    scratch.subdir("a-directory");

    let other = [Leaf::IsType {
        ty: EntryType::Other,
        negate: false,
    }];
    assert!(
        run(&scratch, &other).is_empty(),
        "Windows has no third entry kind, so this must never match"
    );

    let not_other = [Leaf::IsType {
        ty: EntryType::Other,
        negate: true,
    }];
    assert_eq!(run(&scratch, &not_other), ["a-directory", "a-file.txt"]);
}

#[test]
fn is_reparse_negated_matches_ordinary_entries() {
    let scratch = Scratch::with_files(&["ordinary.txt"]);
    let leaves = [Leaf::IsReparse { negate: true }];
    assert_eq!(run(&scratch, &leaves), ["ordinary.txt"]);
}

#[test]
fn a_reparse_point_is_found_by_its_junction_and_matched_by_reparse_tag() {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    let scratch = Scratch::with_files(&["ordinary.txt"]);
    let target = scratch.subdir("target");
    crate::support::create_junction(&scratch.child("junction"), &target);

    let is_reparse = [Leaf::IsReparse { negate: false }];
    assert_eq!(run(&scratch, &is_reparse), ["junction"]);

    let by_tag = [Leaf::ReparseTag {
        tag: IO_REPARSE_TAG_MOUNT_POINT,
        negate: false,
    }];
    assert_eq!(run(&scratch, &by_tag), ["junction"]);

    let not_by_tag = [Leaf::ReparseTag {
        tag: IO_REPARSE_TAG_MOUNT_POINT,
        negate: true,
    }];
    let mut expected = vec!["ordinary.txt".to_string(), "target".to_string()];
    expected.sort();
    assert_eq!(run(&scratch, &not_by_tag), expected);
}

#[test]
fn attribute_mask_leaves_select_by_read_only_status() {
    let scratch = Scratch::with_files(&["writable.txt", "readonly.txt"]);
    let readonly_path = scratch.child("readonly.txt");
    let mut permissions = std::fs::metadata(&readonly_path)
        .expect("metadata")
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&readonly_path, permissions).expect("set readonly");

    const FILE_ATTRIBUTE_READONLY: u32 = 0x0000_0001;

    let all_set = [Leaf::AttrsAllSet(FILE_ATTRIBUTE_READONLY)];
    assert_eq!(run(&scratch, &all_set), ["readonly.txt"]);

    let all_clear = [Leaf::AttrsAllClear(FILE_ATTRIBUTE_READONLY)];
    assert_eq!(run(&scratch, &all_clear), ["writable.txt"]);

    // Windows-only; there is no Unix world-writable bit for clippy's
    // `set_readonly(false)` warning to be about here.
    #[allow(
        clippy::permissions_set_readonly_false,
        reason = "Windows-only; there is no Unix world-writable bit to worry about"
    )]
    {
        let mut permissions = std::fs::metadata(&readonly_path)
            .expect("metadata")
            .permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&readonly_path, permissions).expect("clear readonly");
    }
}

#[test]
fn every_comparison_operator_selects_the_right_files_by_size() {
    let scratch = Scratch::empty();
    std::fs::write(scratch.child("small.dat"), vec![0u8; 10]).expect("a file");
    std::fs::write(scratch.child("medium.dat"), vec![0u8; 100]).expect("a file");
    std::fs::write(scratch.child("large.dat"), vec![0u8; 1000]).expect("a file");

    let cases: &[(Cmp, u64, &[&str])] = &[
        (Cmp::Lt, 100, &["small.dat"]),
        (Cmp::Le, 100, &["small.dat", "medium.dat"]),
        (Cmp::Eq, 100, &["medium.dat"]),
        (Cmp::Ne, 100, &["large.dat", "small.dat"]),
        (Cmp::Ge, 100, &["large.dat", "medium.dat"]),
        (Cmp::Gt, 100, &["large.dat"]),
    ];
    for (op, value, expected) in cases {
        let leaves = [Leaf::Size {
            op: *op,
            value: *value,
        }];
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(run(&scratch, &leaves), expected, "{op:?}");
    }
}

#[test]
fn every_time_field_is_reachable_through_its_own_leaf() {
    let scratch = Scratch::with_files(&["timed.dat"]);
    for field in [
        TimeField::Btime,
        TimeField::Mtime,
        TimeField::Atime,
        TimeField::Ctime,
    ] {
        let leaves = [Leaf::Time {
            field,
            op: Cmp::Ge,
            value: 0,
        }];
        assert_eq!(run(&scratch, &leaves), ["timed.dat"], "{field:?}");
    }
}
