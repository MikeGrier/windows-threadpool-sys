// Copyright (c) 2026 Mike Grier

use std::collections::HashSet;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use super::{NAMED, ShareMode};

/// Every member with the value `windows-sys` gives the flag it is named for. The test that walks
/// this and `NAMED` is what makes the two the same set: a member added to one and not the other fails
/// it.
const EXPECTED: [(&str, u32); 3] = [
    ("READ", FILE_SHARE_READ),
    ("WRITE", FILE_SHARE_WRITE),
    ("DELETE", FILE_SHARE_DELETE),
];

#[test]
fn each_member_is_the_flag_it_is_named_for() {
    for (name, expected) in EXPECTED {
        let (_, member) = NAMED
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("{name} is not a named member"));
        assert_eq!(member.bits(), expected, "{name}");
    }
}

#[test]
fn the_named_members_are_exactly_the_expected_ones() {
    let named: HashSet<&str> = NAMED.iter().map(|(name, _)| *name).collect();
    let expected: HashSet<&str> = EXPECTED.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, expected);
    assert_eq!(NAMED.len(), EXPECTED.len(), "no name is listed twice");
}

#[test]
fn no_two_members_are_the_same_bit_and_none_is_empty() {
    let values: HashSet<u32> = NAMED.iter().map(|(_, member)| member.bits()).collect();
    assert_eq!(values.len(), NAMED.len());
    assert!(NAMED.iter().all(|(_, member)| !member.is_empty()));
    assert!(
        NAMED
            .iter()
            .all(|(_, member)| member.bits().count_ones() == 1),
        "each member is one flag"
    );
}

#[test]
fn none_shares_nothing_and_all_shares_every_member() {
    assert!(ShareMode::NONE.is_empty());
    assert_eq!(ShareMode::NONE.bits(), 0);
    assert!(!ShareMode::ALL.is_empty());
    for (name, member) in NAMED {
        assert!(ShareMode::ALL.contains(*member), "{name}");
        assert!(!ShareMode::NONE.contains(*member), "{name}");
    }
    assert_eq!(
        ShareMode::ALL.bits(),
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        "what every file was opened with before the mode was an option"
    );
}

#[test]
fn a_set_is_joined_with_or_and_asked_with_contains() {
    let share = ShareMode::READ | ShareMode::DELETE;
    assert!(share.contains(ShareMode::READ));
    assert!(share.contains(ShareMode::READ | ShareMode::DELETE));
    assert!(!share.contains(ShareMode::WRITE));
    assert!(!share.contains(ShareMode::WRITE | ShareMode::DELETE));
    assert!(share.contains(ShareMode::NONE), "every set holds nothing");
    assert!(!ShareMode::NONE.contains(ShareMode::READ));

    let mut grown = ShareMode::NONE;
    grown |= ShareMode::WRITE;
    grown |= ShareMode::DELETE;
    assert_eq!(grown, ShareMode::WRITE.union(ShareMode::DELETE));
    assert_eq!(
        grown | grown,
        grown,
        "joining a set with itself changes nothing"
    );
}

#[test]
fn every_subset_of_the_members_is_a_distinct_set_within_all() {
    assert!(NAMED.len() < 16, "an exhaustive walk must stay small");
    let mut seen = HashSet::new();
    for mask in 0..(1u32 << NAMED.len()) {
        let set = NAMED
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .fold(ShareMode::NONE, |set, (_, (_, member))| set | *member);
        assert!(ShareMode::ALL.contains(set), "{set:?}");
        assert!(
            seen.insert(set.bits()),
            "{set:?} is a second way to make a set"
        );
    }
    assert_eq!(seen.len(), 8);
}

#[test]
fn a_set_prints_the_names_of_what_it_holds() {
    assert_eq!(format!("{:?}", ShareMode::NONE), "ShareMode(none)");
    assert_eq!(format!("{:?}", ShareMode::DELETE), "ShareMode(DELETE)");
    assert_eq!(
        format!("{:?}", ShareMode::ALL),
        "ShareMode(READ | WRITE | DELETE)"
    );
    assert_eq!(
        format!("{:?}", ShareMode::DELETE | ShareMode::READ),
        "ShareMode(READ | DELETE)",
        "in the order they are declared, not the order they were joined"
    );
}
