// Copyright (c) 2026 Mike Grier
//! The sizes a declaration gives the core's tables (DI-D-106, DI-D-107), and making a table of one.

use super::{Capacity, ConstructionError, ReserveError, list, queue, table, within, within_queue};

fn with_operations(operations: usize) -> Capacity {
    Capacity {
        operations,
        lineages: 1,
        imports: 0,
        targets: 0,
        domains: 0,
    }
}

#[test]
fn the_flush_parts_are_a_file_part_and_a_part_per_served_domain_for_each_operation() {
    let capacity = with_operations(10);
    assert_eq!(capacity.flush_parts(0), Some(10));
    assert_eq!(capacity.flush_parts(1), Some(20));
    assert_eq!(capacity.flush_parts(3), Some(40));
    assert_eq!(capacity.flush_parts(9), Some(100));
}

#[test]
fn no_operations_leave_no_flush_parts_whatever_is_served() {
    assert_eq!(with_operations(0).flush_parts(0), Some(0));
    assert_eq!(with_operations(0).flush_parts(5), Some(0));
}

#[test]
fn flush_parts_past_a_usize_are_none() {
    assert_eq!(with_operations(usize::MAX).flush_parts(1), None);
    assert_eq!(with_operations(usize::MAX / 2 + 1).flush_parts(1), None);
    assert_eq!(
        with_operations(usize::MAX / 2).flush_parts(1),
        Some(usize::MAX - 1)
    );
    assert_eq!(with_operations(2).flush_parts(usize::MAX), None);
}

#[test]
fn a_list_has_room_for_the_items_asked_for_and_refuses_one_more_unchanged() {
    let mut made: Vec<u32> = list("made", Some(3)).expect("small");
    assert!(made.capacity() >= 3);
    for item in 0..made.capacity() {
        within(&mut made, u32::try_from(item).expect("small")).expect("room");
    }
    let room = made.capacity();
    assert_eq!(within(&mut made, 99), Err(99), "handed back");
    assert_eq!((made.len(), made.capacity()), (room, room), "never grown");
}

#[test]
fn a_queue_has_room_for_the_items_asked_for_and_refuses_one_more_unchanged() {
    let mut made: std::collections::VecDeque<u32> = queue("made", Some(3)).expect("small");
    assert!(made.capacity() >= 3);
    for item in 0..made.capacity() {
        within_queue(&mut made, u32::try_from(item).expect("small")).expect("room");
    }
    let room = made.capacity();
    assert_eq!(within_queue(&mut made, 99), Err(99), "handed back");
    assert_eq!((made.len(), made.capacity()), (room, room), "never grown");
    assert_eq!(made.pop_front(), Some(0), "in the order put");
}

#[test]
fn a_list_with_no_room_refuses_its_first_item() {
    let mut made: Vec<u32> = list("empty", Some(0)).expect("nothing to reserve");
    assert_eq!(within(&mut made, 1), Err(1));
    assert!(made.is_empty());
}

#[test]
fn a_list_or_queue_of_a_size_that_overflowed_is_refused_as_too_large() {
    for refused in [
        list::<u8>("list", None).map(drop),
        queue::<u8>("queue", None).map(drop),
    ] {
        assert!(matches!(
            refused,
            Err(ConstructionError::Table {
                error: ReserveError::TooLarge,
                ..
            })
        ));
    }
}

#[test]
fn a_list_too_large_to_make_is_refused_naming_it() {
    let refused = list::<u64>("the huge list", Some(usize::MAX));
    assert!(matches!(
        refused,
        Err(ConstructionError::Table {
            table: "the huge list",
            error: ReserveError::Alloc(_)
        })
    ));
}

#[test]
fn a_table_holds_at_least_the_entries_asked_for() {
    for entries in [0, 1, 2, 7, 8, 9, 63, 64, 65, 1000] {
        let made = table::<u64, u64>("the test's", Some(entries)).expect("a small table");
        assert!(made.capacity() >= entries, "{entries}");
        assert!(made.is_empty());
    }
}

#[test]
fn a_table_of_a_size_that_overflowed_is_refused_as_too_large() {
    let refused = table::<u64, u64>("the test's", None).expect_err("no size");
    assert_eq!(
        refused,
        ConstructionError::Table {
            table: "the test's",
            error: ReserveError::TooLarge
        }
    );
}

#[test]
fn a_table_too_large_to_make_is_refused_naming_it_and_never_panics() {
    let refused = table::<u64, u64>("the test's", Some(usize::MAX)).expect_err("too large");
    assert!(matches!(
        refused,
        ConstructionError::Table {
            table: "the test's",
            ..
        }
    ));
    assert!(refused.to_string().contains("the test's"));
}
