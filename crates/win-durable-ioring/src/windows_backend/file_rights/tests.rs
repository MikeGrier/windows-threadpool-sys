// Copyright (c) 2026 Mike Grier

use std::collections::HashSet;

use windows_sys::Win32::Foundation::{GENERIC_ALL, GENERIC_EXECUTE, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ALL_ACCESS, FILE_APPEND_DATA, FILE_EXECUTE, FILE_READ_ATTRIBUTES, FILE_READ_DATA,
    FILE_READ_EA, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, READ_CONTROL, SYNCHRONIZE,
    WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;

use super::{COMPOSITES, FileRights, SINGLES};

/// Every single right with the value `windows-sys` gives the constant it is named for. The test
/// that walks this and `SINGLES` is what makes the two the same set: a right added to one and not
/// the other fails it.
const EXPECTED: [(&str, u32); 16] = [
    ("READ_DATA", FILE_READ_DATA),
    ("WRITE_DATA", FILE_WRITE_DATA),
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
    ("GENERIC_READ", GENERIC_READ),
    ("GENERIC_WRITE", GENERIC_WRITE),
    ("GENERIC_EXECUTE", GENERIC_EXECUTE),
];

/// Every named right, single or composite.
fn every_named() -> Vec<(&'static str, FileRights)> {
    SINGLES.iter().chain(COMPOSITES).copied().collect()
}

#[test]
fn each_single_right_is_the_constant_it_is_named_for() {
    for (name, expected) in EXPECTED {
        let (_, right) = SINGLES
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .unwrap_or_else(|| panic!("{name} is not a named right"));
        assert_eq!(right.bits(), expected, "{name}");
    }
}

#[test]
fn the_named_singles_are_exactly_the_expected_ones() {
    // `GENERIC_ALL` is the one single right with no row in `EXPECTED`, so that the table and the
    // list differ by it and by nothing else.
    let named: HashSet<&str> = SINGLES.iter().map(|(name, _)| *name).collect();
    let expected: HashSet<&str> = EXPECTED.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        named.difference(&expected).copied().collect::<Vec<_>>(),
        ["GENERIC_ALL"]
    );
    assert!(expected.is_subset(&named));
    assert_eq!(FileRights::GENERIC_ALL.bits(), GENERIC_ALL);
}

#[test]
fn the_one_composite_is_every_right_of_a_file() {
    assert_eq!(
        COMPOSITES.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["ALL_ACCESS"]
    );
    assert_eq!(FileRights::ALL_ACCESS.bits(), FILE_ALL_ACCESS);
}

#[test]
fn no_two_named_rights_are_the_same_value_and_none_is_empty() {
    let named = every_named();
    let values: HashSet<u32> = named.iter().map(|(_, right)| right.bits()).collect();
    assert_eq!(values.len(), named.len());
    assert!(named.iter().all(|(_, right)| !right.is_empty()));
}

#[test]
fn a_set_is_joined_with_or_and_asked_with_contains() {
    let rights = FileRights::READ_DATA | FileRights::WRITE_DATA | FileRights::READ_ATTRIBUTES;
    assert!(rights.contains(FileRights::READ_DATA));
    assert!(rights.contains(FileRights::READ_DATA | FileRights::READ_ATTRIBUTES));
    assert!(!rights.contains(FileRights::DELETE));
    assert!(!rights.contains(FileRights::READ_DATA | FileRights::DELETE));
    assert!(rights.contains(FileRights::NONE), "every set holds nothing");
    assert!(!FileRights::NONE.contains(FileRights::READ_DATA));

    let mut grown = FileRights::NONE;
    grown |= FileRights::READ_DATA;
    grown |= FileRights::WRITE_DATA;
    assert_eq!(grown, FileRights::READ_DATA.union(FileRights::WRITE_DATA));
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
    assert!(!FileRights::READ_DATA.is_empty());
}

#[test]
fn a_generic_right_is_the_bit_that_was_named_and_not_its_expansion() {
    // The kernel expands a generic right when it opens, so a set holding only one does not contain
    // the specific rights it maps to.
    assert!(!FileRights::GENERIC_READ.contains(FileRights::READ_DATA));
    assert!(!FileRights::GENERIC_WRITE.contains(FileRights::WRITE_DATA));
    assert!(!FileRights::GENERIC_ALL.contains(FileRights::READ_DATA));
}

#[test]
fn a_data_right_is_a_specific_one_or_a_generic_one_that_maps_to_one() {
    let reads = [
        FileRights::READ_DATA,
        FileRights::GENERIC_READ,
        FileRights::GENERIC_ALL,
        FileRights::ALL_ACCESS,
    ];
    let writes = [
        FileRights::WRITE_DATA,
        FileRights::GENERIC_WRITE,
        FileRights::GENERIC_ALL,
        FileRights::ALL_ACCESS,
    ];
    for right in reads {
        assert!(right.grants_read_data(), "{right:?}");
    }
    for right in writes {
        assert!(right.grants_write_data(), "{right:?}");
    }
    // Neither one alone grants the other.
    assert!(!FileRights::READ_DATA.grants_write_data());
    assert!(!FileRights::GENERIC_READ.grants_write_data());
    assert!(!FileRights::WRITE_DATA.grants_read_data());
    assert!(!FileRights::GENERIC_WRITE.grants_read_data());

    // A right that is not a data right is not enough for a file target, nor are several of them.
    let none_of_them = [
        FileRights::NONE,
        FileRights::READ_ATTRIBUTES,
        FileRights::WRITE_ATTRIBUTES,
        FileRights::READ_EA,
        FileRights::WRITE_EA,
        FileRights::EXECUTE,
        FileRights::GENERIC_EXECUTE,
        FileRights::DELETE,
        FileRights::READ_CONTROL,
        FileRights::WRITE_DAC,
        FileRights::WRITE_OWNER,
        FileRights::SYNCHRONIZE,
        FileRights::ACCESS_SYSTEM_SECURITY,
    ];
    let mut all = FileRights::NONE;
    for right in none_of_them {
        assert!(!right.grants_a_data_right(), "{right:?}");
        all |= right;
    }
    assert!(!all.grants_a_data_right(), "{all:?}");
}

#[test]
fn append_only_access_cannot_be_built_from_any_join_of_the_named_rights() {
    // Every join of the 17 named rights, which is every set there is to ask for: whenever the append
    // bit is in it, the right to write at an offset comes with it.
    let named = every_named();
    assert!(named.len() < 20, "an exhaustive walk must stay small");
    for mask in 0..(1u32 << named.len()) {
        let set = named
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .fold(FileRights::NONE, |set, (_, (_, right))| set | *right);
        if set.bits() & FILE_APPEND_DATA != 0 {
            assert!(
                set.grants_write_data(),
                "append without write at an offset: {set:?}"
            );
        }
    }
}

#[test]
fn no_member_is_the_append_bit_on_its_own() {
    for (name, right) in every_named() {
        assert_ne!(right.bits(), FILE_APPEND_DATA, "{name}");
    }
}

#[test]
fn a_set_prints_the_names_of_what_it_holds() {
    assert_eq!(format!("{:?}", FileRights::NONE), "FileRights(none)");
    assert_eq!(
        format!("{:?}", FileRights::READ_DATA | FileRights::WRITE_DATA),
        "FileRights(READ_DATA | WRITE_DATA)"
    );
    assert_eq!(
        format!("{:?}", FileRights::GENERIC_READ | FileRights::GENERIC_WRITE),
        "FileRights(GENERIC_READ | GENERIC_WRITE)"
    );
    assert_eq!(
        format!("{:?}", FileRights::ALL_ACCESS),
        "FileRights(ALL_ACCESS)"
    );
}

#[test]
fn bits_no_single_names_are_printed_in_hex() {
    // `ALL_ACCESS` carries the append and delete-child bits, which have no member of their own, so a
    // set that is that and one more right is the names of the ones it can and the rest as a number.
    let printed = format!(
        "{:?}",
        FileRights::ALL_ACCESS | FileRights::ACCESS_SYSTEM_SECURITY
    );
    assert!(printed.contains("ACCESS_SYSTEM_SECURITY"), "{printed}");
    assert!(printed.contains("READ_DATA"), "{printed}");
    assert!(printed.ends_with("| 0x00000044)"), "{printed}");
}
