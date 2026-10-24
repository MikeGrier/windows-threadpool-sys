// Copyright (c) 2026 Mike Grier
//! The sizes a declaration gives the core's tables (DI-D-106, DI-D-107), and making a table of one.

use super::{Capacity, ConstructionError, ReserveError, table};

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
