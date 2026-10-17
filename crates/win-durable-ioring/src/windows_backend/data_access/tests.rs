// Copyright (c) 2026 Mike Grier

use std::any::TypeId;

use windows_sys::Win32::Storage::FileSystem::{FILE_APPEND_DATA, FILE_READ_DATA, FILE_WRITE_DATA};

use super::{CanRead, CanWrite, DataAccess, HasData, NoData, ReadOnly, ReadWrite, WriteOnly};

fn id<T: 'static>() -> TypeId {
    TypeId::of::<T>()
}

/// One row for each marker: what it asks of the open, whether it reads and writes, and what adding
/// each axis makes of it.
struct Row {
    name: &'static str,
    rights: u32,
    reads: bool,
    writes: bool,
    with_read: TypeId,
    with_write: TypeId,
}

fn row<A: DataAccess>(name: &'static str, reads: bool, writes: bool) -> Row {
    Row {
        name,
        rights: A::RIGHTS,
        reads,
        writes,
        with_read: id::<A::WithRead>(),
        with_write: id::<A::WithWrite>(),
    }
}

fn rows() -> [Row; 4] {
    [
        row::<NoData>("NoData", false, false),
        row::<ReadOnly>("ReadOnly", true, false),
        row::<WriteOnly>("WriteOnly", false, true),
        row::<ReadWrite>("ReadWrite", true, true),
    ]
}

#[test]
fn each_marker_asks_for_exactly_the_data_rights_it_names() {
    for row in rows() {
        let expected =
            u32::from(row.reads) * FILE_READ_DATA + u32::from(row.writes) * FILE_WRITE_DATA;
        assert_eq!(row.rights, expected, "{}", row.name);
    }
}

#[test]
fn no_marker_asks_for_the_append_bit() {
    for row in rows() {
        assert_eq!(row.rights & FILE_APPEND_DATA, 0, "{}", row.name);
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
            assert_ne!(row.rights, other.rights, "{} and {}", row.name, other.name);
        }
    }
}

fn has_data<A: HasData>() {}
fn can_read<A: CanRead>() {}
fn can_write<A: CanWrite>() {}

#[test]
fn the_capability_traits_are_implemented_for_the_markers_that_have_the_capability() {
    has_data::<ReadOnly>();
    has_data::<WriteOnly>();
    has_data::<ReadWrite>();
    can_read::<ReadOnly>();
    can_read::<ReadWrite>();
    can_write::<WriteOnly>();
    can_write::<ReadWrite>();
}

// What a marker lacks is refused at compile time, and those refusals are the `compile_fail` doctests
// on the three traits in `data_access.rs`.
