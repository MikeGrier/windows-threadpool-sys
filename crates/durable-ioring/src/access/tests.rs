// Copyright (c) 2026 Mike Grier

use std::any::TypeId;

use super::{
    Access, CanRead, CanWrite, DataAccess, HasData, NoData, ReadOnly, ReadWrite, WriteOnly,
};

fn id<T: 'static>() -> TypeId {
    TypeId::of::<T>()
}

/// One row for each marker: whether it reads and writes, and what adding each axis makes of it.
struct Row {
    name: &'static str,
    reads: bool,
    writes: bool,
    with_read: TypeId,
    with_write: TypeId,
}

fn row<A: DataAccess>(name: &'static str) -> Row {
    Row {
        name,
        reads: A::READ,
        writes: A::WRITE,
        with_read: id::<A::WithRead>(),
        with_write: id::<A::WithWrite>(),
    }
}

fn rows() -> [Row; 4] {
    [
        row::<NoData>("NoData"),
        row::<ReadOnly>("ReadOnly"),
        row::<WriteOnly>("WriteOnly"),
        row::<ReadWrite>("ReadWrite"),
    ]
}

#[test]
fn each_marker_reads_and_writes_as_it_is_named() {
    let expected = [
        ("NoData", false, false),
        ("ReadOnly", true, false),
        ("WriteOnly", false, true),
        ("ReadWrite", true, true),
    ];
    for (row, (name, reads, writes)) in rows().iter().zip(expected) {
        assert_eq!(row.name, name);
        assert_eq!((row.reads, row.writes), (reads, writes), "{name}");
    }
}

#[test]
fn adding_an_axis_gives_the_marker_that_has_it_and_what_it_already_had() {
    let markers = [
        (id::<NoData>(), false, false),
        (id::<ReadOnly>(), true, false),
        (id::<WriteOnly>(), false, true),
        (id::<ReadWrite>(), true, true),
    ];
    let marker = |reads: bool, writes: bool| {
        markers
            .iter()
            .find(|(_, r, w)| *r == reads && *w == writes)
            .map(|(marker, _, _)| *marker)
            .expect("one marker for each combination")
    };
    for row in rows() {
        assert_eq!(
            row.with_read,
            marker(true, row.writes),
            "{} + read",
            row.name
        );
        assert_eq!(
            row.with_write,
            marker(row.reads, true),
            "{} + write",
            row.name
        );
    }
}

#[test]
fn the_four_markers_are_four_different_combinations_of_reading_and_writing() {
    let rows = rows();
    for (index, row) in rows.iter().enumerate() {
        for other in &rows[index + 1..] {
            assert_ne!(
                (row.reads, row.writes),
                (other.reads, other.writes),
                "{} and {}",
                row.name,
                other.name
            );
        }
    }
}

fn has_data<A: HasData>() -> Access {
    A::ACCESS
}
fn can_read<A: CanRead>() {}
fn can_write<A: CanWrite>() {}

#[test]
fn a_marker_with_data_names_the_access_a_target_made_from_it_states() {
    assert_eq!(has_data::<ReadOnly>(), Access::ReadOnly);
    assert_eq!(has_data::<WriteOnly>(), Access::WriteOnly);
    assert_eq!(has_data::<ReadWrite>(), Access::ReadWrite);
}

#[test]
fn the_stated_access_reads_and_writes_as_the_marker_it_stands_for_does() {
    fn agrees<A: HasData>() {
        assert_eq!(A::ACCESS.can_read(), A::READ);
        assert_eq!(A::ACCESS.can_write(), A::WRITE);
    }
    agrees::<ReadOnly>();
    agrees::<WriteOnly>();
    agrees::<ReadWrite>();
}

#[test]
fn an_access_can_read_and_write_as_it_is_named() {
    assert!(Access::ReadOnly.can_read() && !Access::ReadOnly.can_write());
    assert!(!Access::WriteOnly.can_read() && Access::WriteOnly.can_write());
    assert!(Access::ReadWrite.can_read() && Access::ReadWrite.can_write());
}

#[test]
fn an_access_prints_what_it_allows() {
    assert_eq!(Access::ReadOnly.to_string(), "read only");
    assert_eq!(Access::WriteOnly.to_string(), "write only");
    assert_eq!(Access::ReadWrite.to_string(), "read and write");
}

#[test]
fn the_capability_traits_are_implemented_for_the_markers_that_have_the_capability() {
    can_read::<ReadOnly>();
    can_read::<ReadWrite>();
    can_write::<WriteOnly>();
    can_write::<ReadWrite>();
}

// What a marker lacks is refused at compile time, and those refusals are the `compile_fail` doctests
// on the three traits in `access.rs`.
