// Copyright (c) 2026 Mike Grier

use crate::contract::{EpochId, Identities};
use crate::ids::DioringIds;

/// A consumer's own epoch-id type with no `Hash`: the identities must not need one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Lsn(u64);

fn assert_epoch_id<T: EpochId>() {}
fn assert_identities<V: Identities>() {}

#[test]
fn every_copy_ord_debug_type_is_an_epoch_id() {
    assert_epoch_id::<u64>();
    assert_epoch_id::<u32>();
    assert_epoch_id::<Lsn>();
    assert_epoch_id::<(u32, u64)>();
    assert_epoch_id::<char>();
}

#[test]
fn dioring_names_its_identities_for_any_epoch_id_type() {
    assert_identities::<DioringIds<u64, u64>>();
    assert_identities::<DioringIds<Lsn, u64>>();
    assert_identities::<DioringIds<(u32, u64), u64>>();
}
