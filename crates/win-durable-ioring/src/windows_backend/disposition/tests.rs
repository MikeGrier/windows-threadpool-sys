// Copyright (c) 2026 Mike Grier

use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, OPEN_ALWAYS, OPEN_EXISTING};

use super::Disposition;

#[test]
fn each_variant_is_the_value_it_is_named_for() {
    assert_eq!(Disposition::CreateNew.bits(), CREATE_NEW);
    assert_eq!(Disposition::OpenExisting.bits(), OPEN_EXISTING);
    assert_eq!(Disposition::OpenAlways.bits(), OPEN_ALWAYS);
}

#[test]
fn the_three_variants_are_three_different_values() {
    let all = [
        Disposition::CreateNew,
        Disposition::OpenExisting,
        Disposition::OpenAlways,
    ];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a.bits(), b.bits(), "{a:?} and {b:?}");
        }
    }
}
