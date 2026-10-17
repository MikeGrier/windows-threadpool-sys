// Copyright (c) 2026 Mike Grier

use std::collections::HashSet;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_TEMPORARY,
};

use super::{FileAttributes, NAMED};

/// Every member with the value `windows-sys` gives the attribute it is named for. The test that walks
/// this and `NAMED` is what makes the two the same set.
const EXPECTED: [(&str, u32); 6] = [
    ("ARCHIVE", FILE_ATTRIBUTE_ARCHIVE),
    ("HIDDEN", FILE_ATTRIBUTE_HIDDEN),
    ("OFFLINE", FILE_ATTRIBUTE_OFFLINE),
    ("READONLY", FILE_ATTRIBUTE_READONLY),
    ("SYSTEM", FILE_ATTRIBUTE_SYSTEM),
    ("TEMPORARY", FILE_ATTRIBUTE_TEMPORARY),
];

#[test]
fn each_member_is_the_attribute_it_is_named_for() {
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
        "each member is one attribute"
    );
}

#[test]
fn normal_and_encrypted_are_not_members() {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_NORMAL,
    };
    for (name, member) in NAMED {
        assert_ne!(member.bits(), FILE_ATTRIBUTE_NORMAL, "{name}");
        assert_ne!(member.bits(), FILE_ATTRIBUTE_ENCRYPTED, "{name}");
    }
}

#[test]
fn none_is_empty_and_is_not_a_member() {
    assert!(FileAttributes::NONE.is_empty());
    assert_eq!(FileAttributes::NONE.bits(), 0);
    for (name, member) in NAMED {
        assert!(!FileAttributes::NONE.contains(*member), "{name}");
    }
}

#[test]
fn a_set_is_joined_with_or_and_asked_with_contains() {
    let set = FileAttributes::HIDDEN | FileAttributes::SYSTEM;
    assert!(set.contains(FileAttributes::HIDDEN));
    assert!(set.contains(FileAttributes::HIDDEN | FileAttributes::SYSTEM));
    assert!(!set.contains(FileAttributes::READONLY));
    assert!(!set.contains(FileAttributes::READONLY | FileAttributes::SYSTEM));
    assert!(
        set.contains(FileAttributes::NONE),
        "every set holds nothing"
    );

    let mut grown = FileAttributes::NONE;
    grown |= FileAttributes::TEMPORARY;
    grown |= FileAttributes::OFFLINE;
    assert_eq!(
        grown,
        FileAttributes::TEMPORARY.union(FileAttributes::OFFLINE)
    );
    assert_eq!(grown | grown, grown);
}

#[test]
fn every_subset_of_the_members_is_a_distinct_set() {
    assert!(NAMED.len() < 16, "an exhaustive walk must stay small");
    let mut seen = HashSet::new();
    for mask in 0..(1u32 << NAMED.len()) {
        let set = NAMED
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .fold(FileAttributes::NONE, |set, (_, (_, member))| set | *member);
        assert!(
            seen.insert(set.bits()),
            "{set:?} is a second way to make a set"
        );
    }
    assert_eq!(seen.len(), 64);
}

#[test]
fn a_set_prints_the_names_of_what_it_holds() {
    assert_eq!(
        format!("{:?}", FileAttributes::NONE),
        "FileAttributes(none)"
    );
    assert_eq!(
        format!("{:?}", FileAttributes::HIDDEN),
        "FileAttributes(HIDDEN)"
    );
    assert_eq!(
        format!("{:?}", FileAttributes::SYSTEM | FileAttributes::ARCHIVE),
        "FileAttributes(ARCHIVE | SYSTEM)",
        "in the order they are declared, not the order they were joined"
    );
}
