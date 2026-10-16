// Copyright (c) 2026 Mike Grier
//! What `decode` does with a reply, against replies built from the structure's declared layout.

use super::*;

// --- What the entry does with a reply -------------------------------------------------------

#[test]
fn a_version_two_entry_has_every_field_read_from_where_the_structure_has_it() {
    let info = decode_built(2, &[distinct(1)]).expect("a whole reply");
    let entry = &info.entries()[0];
    let wanted = distinct(1);

    assert_eq!(info.version(), StorageDependencyVersion::V2);
    assert_eq!(info.entries().len(), 1);
    assert_eq!(entry.dependency_type_flags().bits(), wanted.type_flags);
    assert_eq!(entry.provider_specific_flags(), wanted.provider);
    assert_eq!(entry.virtual_storage_type().device_id(), wanted.device_id);
    assert_eq!(entry.virtual_storage_type().vendor_id(), wanted.vendor);
    assert_eq!(entry.ancestor_level(), Some(wanted.ancestor));
    assert_eq!(
        entry.dependency_device_name(),
        name(wanted.names[0].expect("set")).as_ref()
    );
    assert_eq!(
        entry.host_volume_name(),
        name(wanted.names[1].expect("set")).as_ref()
    );
    assert_eq!(
        entry.dependent_volume_name(),
        name(wanted.names[2].expect("set")).as_ref()
    );
    assert_eq!(
        entry.dependent_volume_relative_path(),
        name(wanted.names[3].expect("set")).as_ref()
    );
}

#[test]
fn a_version_one_entry_has_its_fields_and_no_names() {
    let info = decode_built(1, &[distinct(2), distinct(3)]).expect("a whole reply");

    assert_eq!(info.version(), StorageDependencyVersion::V1);
    assert_eq!(info.entries().len(), 2);
    for (entry, seed) in info.entries().iter().zip([2, 3]) {
        let wanted = distinct(seed);
        assert_eq!(entry.dependency_type_flags().bits(), wanted.type_flags);
        assert_eq!(entry.provider_specific_flags(), wanted.provider);
        assert_eq!(entry.virtual_storage_type().device_id(), wanted.device_id);
        assert_eq!(entry.virtual_storage_type().vendor_id(), wanted.vendor);
        assert_eq!(entry.ancestor_level(), None);
        assert_eq!(entry.dependency_device_name(), None);
        assert_eq!(entry.host_volume_name(), None);
        assert_eq!(entry.dependent_volume_name(), None);
        assert_eq!(entry.dependent_volume_relative_path(), None);
    }
}

#[test]
fn several_version_two_entries_are_each_read_at_their_own_offset_with_their_own_names() {
    let entries: Vec<Built> = (1..=5).map(distinct).collect();
    let info = decode_built(2, &entries).expect("a whole reply");

    assert_eq!(info.entries().len(), 5);
    for (entry, wanted) in info.entries().iter().zip(&entries) {
        assert_eq!(entry.dependency_type_flags().bits(), wanted.type_flags);
        assert_eq!(entry.virtual_storage_type().device_id(), wanted.device_id);
        assert_eq!(entry.ancestor_level(), Some(wanted.ancestor));
        assert_eq!(
            entry.host_volume_name(),
            name(wanted.names[1].expect("set")).as_ref()
        );
    }
}

#[test]
fn a_null_name_is_none_and_the_others_are_still_read() {
    for missing in 0..4 {
        let mut built_entry = distinct(4);
        built_entry.names[missing] = None;
        let info = decode_built(2, &[built_entry.clone()]).expect("a whole reply");
        let entry = &info.entries()[0];

        let got = [
            entry.dependency_device_name(),
            entry.host_volume_name(),
            entry.dependent_volume_name(),
            entry.dependent_volume_relative_path(),
        ];
        for (slot, (got, wanted)) in got.into_iter().zip(built_entry.names).enumerate() {
            assert_eq!(
                got,
                wanted.and_then(name).as_ref(),
                "name {slot}, with {missing} null"
            );
        }
    }
}

#[test]
fn an_empty_name_is_an_empty_string_and_not_none() {
    let mut built_entry = distinct(5);
    built_entry.names = [Some(""), None, None, None];
    let info = decode_built(2, &[built_entry]).expect("a whole reply");

    assert_eq!(
        info.entries()[0].dependency_device_name(),
        name("").as_ref()
    );
}

#[test]
fn a_name_with_characters_outside_the_basic_plane_is_carried_as_the_units_the_device_wrote() {
    let mut built_entry = distinct(6);
    built_entry.names = [Some("disk-\u{1F4BE}-\u{00E9}"), None, None, None];
    let info = decode_built(2, &[built_entry]).expect("a whole reply");

    assert_eq!(
        info.entries()[0].dependency_device_name(),
        name("disk-\u{1F4BE}-\u{00E9}").as_ref()
    );
}

#[test]
fn a_reply_that_holds_no_entries_is_empty_and_not_an_error() {
    for version in [1, 2] {
        let info = decode_built(version, &[]).expect("no entries");
        assert!(info.entries().is_empty());
        assert_eq!(info.version().raw(), version);
    }
}

#[test]
fn flag_bits_are_carried_as_the_device_wrote_them_including_the_top_one() {
    for bits in [0, 1, 64 + 256, 0x3FFF, 0x4000, i32::MIN, -1] {
        let mut built_entry = distinct(7);
        built_entry.type_flags = bits;
        let info = decode_built(1, &[built_entry]).expect("a whole reply");
        assert_eq!(info.entries()[0].dependency_type_flags().bits(), bits);
    }
}

#[test]
fn a_name_pointer_outside_the_reply_is_reported_and_not_read() {
    // Each of these is an address a hostile or damaged reply could hold, for the first name.
    let slot = FIRST_ENTRY + offset_of!(STORAGE_DEPENDENCY_INFO_TYPE_2, DependencyDeviceName);

    for (what, pick) in [
        (
            "below the reply",
            (|base: usize, _len: usize| base - 8) as fn(usize, usize) -> usize,
        ),
        ("at its end", |base, len| base + len),
        ("past its end", |base, len| base + len + 4096),
        ("at address 1", |_, _| 1),
        ("at the top of the address space", |_, _| usize::MAX),
    ] {
        let mut buffer = built(2, &[distinct(8)], 0);
        let base = buffer.as_slice().as_ptr().addr();
        let len = buffer.len();
        put(buffer.as_mut_slice(), slot, &pick(base, len).to_ne_bytes());

        eprintln!("a name {what}");
        assert_malformed(
            decode(buffer.as_slice(), u32::try_from(len).expect("small")),
            "does not lie",
        );
    }
}

#[test]
fn a_name_that_runs_to_the_end_of_the_reply_without_a_terminator_is_reported() {
    // The last name's terminator is overwritten with a letter, so it runs to the end.
    let mut buffer = built(2, &[distinct(9)], 0);
    let used = buffer.len();
    put(
        buffer.as_mut_slice(),
        used - 2,
        &u16::from(b'x').to_ne_bytes(),
    );

    assert_malformed(
        decode(buffer.as_slice(), u32::try_from(used).expect("small")),
        "no terminator",
    );
}

#[test]
fn a_name_whose_terminator_lies_beyond_what_the_call_used_is_reported() {
    // The bytes are there and the terminator is in them, but the call said it used fewer: what
    // it did not claim to have written is not read.
    let buffer = built(2, &[distinct(9)], 8);
    let used = needed(2, &[distinct(9)]);

    assert!(decode(buffer.as_slice(), u32::try_from(used).expect("small")).is_ok());
    for short in [1, 2, 3] {
        eprintln!("used {short} short");
        assert_malformed(
            decode(
                buffer.as_slice(),
                u32::try_from(used - short).expect("small"),
            ),
            "no terminator",
        );
    }
}

#[test]
fn a_name_that_ends_exactly_at_the_end_of_the_reply_is_read() {
    // The terminator is the last two bytes used: the boundary the check above is one past.
    let buffer = built(2, &[distinct(10)], 0);
    assert_eq!(buffer.len(), needed(2, &[distinct(10)]));

    let info = decode(
        buffer.as_slice(),
        u32::try_from(buffer.len()).expect("small"),
    )
    .expect("a terminator in the last two bytes");
    assert_eq!(
        info.entries()[0].dependent_volume_relative_path(),
        name(r"images\disk.vhdx").as_ref()
    );
}

#[test]
fn a_count_the_reply_cannot_hold_is_reported_and_not_read() {
    let entries = [distinct(11), distinct(12)];
    let len = built(2, &entries, 0).len();
    // The most entries whose own bytes fit in the reply, names and all, and the next one.
    let fits = (len - FIRST_ENTRY) / ENTRY_2;
    let fits = u32::try_from(fits).expect("small");

    for count in [fits + 1, fits + 2, 1000, u32::MAX] {
        let mut buffer = built(2, &entries, 0);
        put(
            buffer.as_mut_slice(),
            offset_of!(STORAGE_DEPENDENCY_INFO, NumberEntries),
            &count.to_ne_bytes(),
        );
        let used = u32::try_from(buffer.len()).expect("small");

        eprintln!("a count of {count}");
        assert_malformed(decode(buffer.as_slice(), used), "count");
    }
}

#[test]
fn a_count_that_fits_exactly_is_read_and_one_more_is_not() {
    // A version 1 reply with no names: the bytes are exactly the header and the entries.
    let entries = [distinct(13), distinct(14)];
    let mut buffer = built(1, &entries, 0);
    let end = FIRST_ENTRY + 2 * ENTRY_1;
    let used = u32::try_from(end).expect("small");
    assert!(buffer.len() >= end);

    assert!(decode(buffer.as_slice(), used).is_ok());
    put(
        buffer.as_mut_slice(),
        offset_of!(STORAGE_DEPENDENCY_INFO, NumberEntries),
        &3_u32.to_ne_bytes(),
    );
    assert_malformed(decode(buffer.as_slice(), used), "count");
}

#[test]
fn a_reply_in_a_version_this_entry_does_not_know_is_reported_and_not_read() {
    for version in [0_i32, 3, 4, -1, i32::MAX] {
        let mut buffer = built(2, &[distinct(15)], 0);
        put(
            buffer.as_mut_slice(),
            offset_of!(STORAGE_DEPENDENCY_INFO, Version),
            &version.to_ne_bytes(),
        );
        let used = u32::try_from(buffer.len()).expect("small");

        assert!(
            matches!(
                decode(buffer.as_slice(), used),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ),
            "version {version}"
        );
    }
}

#[test]
fn a_version_too_large_to_be_one_is_reported() {
    let mut buffer = built(2, &[], 0);
    put(
        buffer.as_mut_slice(),
        offset_of!(STORAGE_DEPENDENCY_INFO, Version),
        &u32::MAX.to_ne_bytes(),
    );
    let used = u32::try_from(buffer.len()).expect("small");

    assert!(matches!(
        decode(buffer.as_slice(), used),
        Err(error) if error.kind() == io::ErrorKind::InvalidData
    ));
}

#[test]
fn a_call_that_used_more_than_it_was_given_is_reported() {
    let buffer = built(2, &[], 0);
    let len = u32::try_from(buffer.len()).expect("small");

    assert!(decode(buffer.as_slice(), len).is_ok());
    for used in [len + 1, len + 4096, u32::MAX] {
        assert!(
            matches!(
                decode(buffer.as_slice(), used),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ),
            "used {used} of {len}"
        );
    }
}

#[test]
fn a_reply_cut_short_inside_its_header_is_reported() {
    let buffer = built(2, &[], 0);

    for used in 0..FIRST_ENTRY {
        assert!(
            matches!(
                decode(buffer.as_slice(), u32::try_from(used).expect("small")),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ),
            "used {used}"
        );
    }
}

#[test]
fn an_entry_cut_short_is_reported_wherever_it_is_cut() {
    // Version 1 has no names, so every shorter `used` cuts an entry or the count's room for it.
    let buffer = built(1, &[distinct(16)], 0);
    let end = FIRST_ENTRY + ENTRY_1;

    for used in FIRST_ENTRY..end {
        assert!(
            matches!(
                decode(buffer.as_slice(), u32::try_from(used).expect("small")),
                Err(error) if error.kind() == io::ErrorKind::InvalidData
            ),
            "used {used} of {end}"
        );
    }
    assert!(decode(buffer.as_slice(), u32::try_from(end).expect("small")).is_ok());
}
