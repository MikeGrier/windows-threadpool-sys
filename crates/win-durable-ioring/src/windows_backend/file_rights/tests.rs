// Copyright (c) 2026 Mike Grier

use std::collections::HashSet;

use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_EXECUTE, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_READ_EA,
    FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, READ_CONTROL, SYNCHRONIZE, WRITE_DAC,
    WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;

use super::{FileRights, NAMED};

/// Every right with the value `windows-sys` gives the constant it is named for. The test that walks
/// this and `NAMED` is what makes the two the same set: a right added to one and not the other fails
/// it.
const EXPECTED: [(&str, u32); 11] = [
    ("READ_ATTRIBUTES", FILE_READ_ATTRIBUTES),
    ("WRITE_ATTRIBUTES", FILE_WRITE_ATTRIBUTES),
    ("READ_EA", FILE_READ_EA),
    ("WRITE_EA", FILE_WRITE_EA),
    ("EXECUTE", FILE_EXECUTE),
    ("DELETE", DELETE),
    ("READ_CONTROL", READ_CONTROL),
    ("WRITE_DAC", WRITE_DAC),
    ("WRITE_OWNER", WRITE_OWNER),
    ("SYNCHRONIZE", SYNCHRONIZE),
    ("ACCESS_SYSTEM_SECURITY", ACCESS_SYSTEM_SECURITY),
];

#[test]
fn each_right_is_the_constant_it_is_named_for() {
    for (name, expected) in EXPECTED {
        let (_, right) = NAMED
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("{name} is not a named right"));
        assert_eq!(right.bits(), expected, "{name}");
    }
}

#[test]
fn the_named_rights_are_exactly_the_expected_ones() {
    let named: HashSet<&str> = NAMED.iter().map(|(name, _)| *name).collect();
    let expected: HashSet<&str> = EXPECTED.iter().map(|(name, _)| *name).collect();
    assert_eq!(named, expected);
    assert_eq!(NAMED.len(), EXPECTED.len(), "no name is listed twice");
}

#[test]
fn no_two_rights_are_the_same_value_and_none_is_empty() {
    let values: HashSet<u32> = NAMED.iter().map(|(_, right)| right.bits()).collect();
    assert_eq!(values.len(), NAMED.len());
    assert!(NAMED.iter().all(|(_, right)| !right.is_empty()));
}

#[test]
fn a_set_is_joined_with_or_and_asked_with_contains() {
    let rights = FileRights::READ_ATTRIBUTES | FileRights::WRITE_ATTRIBUTES | FileRights::DELETE;
    assert!(rights.contains(FileRights::DELETE));
    assert!(rights.contains(FileRights::READ_ATTRIBUTES | FileRights::DELETE));
    assert!(!rights.contains(FileRights::WRITE_EA));
    assert!(!rights.contains(FileRights::DELETE | FileRights::WRITE_EA));
    assert!(rights.contains(FileRights::NONE), "every set holds nothing");
    assert!(!FileRights::NONE.contains(FileRights::DELETE));

    let mut grown = FileRights::NONE;
    grown |= FileRights::DELETE;
    grown |= FileRights::WRITE_DAC;
    assert_eq!(grown, FileRights::DELETE.union(FileRights::WRITE_DAC));
    assert_eq!(
        grown | grown,
        grown,
        "joining a set with itself changes nothing"
    );
}

#[test]
fn the_empty_set_is_the_default_and_the_only_empty_one() {
    assert_eq!(FileRights::default(), FileRights::NONE);
    assert!(FileRights::NONE.is_empty());
    assert!(!FileRights::DELETE.is_empty());
}

#[test]
fn no_right_is_a_data_right_and_none_is_the_append_bit() {
    // The data rights are the data access type, so a member that carried one would make a handle's
    // type disagree with what it was opened with.
    let data = FILE_READ_DATA | FILE_WRITE_DATA | FILE_APPEND_DATA;
    for (name, right) in NAMED {
        assert_eq!(right.bits() & data, 0, "{name}");
    }
}

#[test]
fn no_join_of_the_rights_carries_a_data_right_or_the_append_bit() {
    // Every join of the named rights, which is every set there is to ask for.
    let data = FILE_READ_DATA | FILE_WRITE_DATA | FILE_APPEND_DATA;
    assert!(NAMED.len() < 16, "an exhaustive walk must stay small");
    for mask in 0..(1u32 << NAMED.len()) {
        let set = NAMED
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .fold(FileRights::NONE, |set, (_, (_, right))| set | *right);
        assert_eq!(set.bits() & data, 0, "{set:?}");
    }
}

#[test]
fn a_set_prints_the_names_of_what_it_holds() {
    assert_eq!(format!("{:?}", FileRights::NONE), "FileRights(none)");
    assert_eq!(format!("{:?}", FileRights::DELETE), "FileRights(DELETE)");
    assert_eq!(
        format!(
            "{:?}",
            FileRights::WRITE_ATTRIBUTES | FileRights::READ_ATTRIBUTES
        ),
        "FileRights(READ_ATTRIBUTES | WRITE_ATTRIBUTES)",
        "in the order they are declared, not the order they were joined"
    );
}
